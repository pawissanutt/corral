import { createConnection, type Socket } from "node:net";

import { within } from "./wait.ts";

export interface Reply {
  readonly id: string | null;
  readonly generation: number;
  readonly ok: boolean;
  readonly result?: Record<string, unknown>;
  readonly error?: { code: string; reason: string };
}

export interface TestClient {
  request(frame: Record<string, unknown>): Promise<Reply>;
  raw(line: string): Promise<Reply>;
  closed(): Promise<void>;
  close(): void;
}

export async function connect(path: string): Promise<TestClient> {
  const socket: Socket = createConnection(path);
  const waiting: ((reply: Reply) => void)[] = [];
  const unsolicited: Reply[] = [];
  let buffer = "";
  const closed = new Promise<void>((resolve) => socket.once("close", () => resolve()));
  socket.on("data", (chunk: Buffer) => {
    buffer += chunk.toString("utf8");
    let boundary: number;
    while ((boundary = buffer.indexOf("\n")) !== -1) {
      const reply = JSON.parse(buffer.slice(0, boundary)) as Reply;
      buffer = buffer.slice(boundary + 1);
      const next = waiting.shift();
      if (next) next(reply);
      else unsolicited.push(reply);
    }
  });
  socket.on("error", () => socket.destroy());
  await new Promise<void>((resolve, reject) => {
    socket.once("connect", () => resolve());
    socket.once("error", reject);
  });
  const next = () => {
    const ready = unsolicited.shift();
    return ready ? Promise.resolve(ready) : within(new Promise<Reply>((resolve) => waiting.push(resolve)), 30_000, "a bridge reply");
  };
  const raw = (line: string) => {
    const reply = next();
    socket.write(line.endsWith("\n") ? line : `${line}\n`);
    return reply;
  };
  return {
    raw,
    request: (frame) => raw(JSON.stringify(frame)),
    closed: () => within(closed, 30_000, "the bridge to close the connection"),
    close: () => socket.destroy(),
  };
}

export async function refusalOnConnect(path: string): Promise<{ reply: Reply; closed: Promise<void> }> {
  const socket = createConnection(path);
  const closed = new Promise<void>((resolve) => socket.once("close", () => resolve()));
  const reply = new Promise<Reply>((resolve) => {
    let buffer = "";
    socket.on("data", (chunk: Buffer) => {
      buffer += chunk.toString("utf8");
      const boundary = buffer.indexOf("\n");
      if (boundary !== -1) resolve(JSON.parse(buffer.slice(0, boundary)) as Reply);
    });
  });
  socket.on("error", () => socket.destroy());
  return { reply: await within(reply, 30_000, "the bridge's refusal"), closed: within(closed, 30_000, "the close") };
}
