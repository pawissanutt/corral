import { createConnection, type Socket } from "node:net";

import { LineReader } from "../../src/bridge/framing.ts";
import { within } from "./wait.ts";

export interface Reply {
  readonly id: string | null;
  readonly generation: number;
  readonly ok: boolean;
  readonly result?: Record<string, unknown>;
  readonly error?: { code: string; reason: string };
  /** Bytes of the reply line on the wire, newline included. */
  readonly wireBytes: number;
}

export interface TestClient {
  request(frame: Record<string, unknown>): Promise<Reply>;
  raw(line: string): Promise<Reply>;
  /** Writes one frame as separate socket writes, split at the given byte offsets. */
  split(frame: Record<string, unknown>, offsets: readonly number[]): Promise<Reply>;
  /** Writes several frames in one socket write and reads one reply per frame. */
  batch(frames: readonly Record<string, unknown>[]): Promise<Reply[]>;
  /**
   * Writes `first` plus `second`'s bytes before `cut` in one write, and the rest of `second` only
   * after `first`'s reply arrived: the server has then already read the prefix on its own.
   */
  pipelined(first: Record<string, unknown>, second: Record<string, unknown>, cut: number): Promise<[Reply, Reply]>;
  closed(): Promise<void>;
  close(): void;
}

function parse(line: string): Reply {
  return { ...(JSON.parse(line) as Omit<Reply, "wireBytes">), wireBytes: Buffer.byteLength(line) + 1 };
}

export async function connect(path: string): Promise<TestClient> {
  const socket: Socket = createConnection(path);
  const reader = new LineReader(1 << 20);
  const waiting: ((reply: Reply) => void)[] = [];
  const unsolicited: Reply[] = [];
  const closed = new Promise<void>((resolve) => socket.once("close", () => resolve()));
  socket.on("data", (chunk: Buffer) => {
    for (const line of reader.push(chunk)) {
      const reply = parse(line);
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
    split(frame, offsets) {
      const bytes = Buffer.from(`${JSON.stringify(frame)}\n`);
      const reply = next();
      let start = 0;
      for (const offset of [...offsets, bytes.length]) {
        socket.write(bytes.subarray(start, offset));
        start = offset;
      }
      return reply;
    },
    async pipelined(first, second, cut) {
      const bytes = Buffer.from(`${JSON.stringify(second)}\n`);
      const firstReply = next();
      socket.write(Buffer.concat([Buffer.from(`${JSON.stringify(first)}\n`), bytes.subarray(0, cut)]));
      const answered = await firstReply;
      const secondReply = next();
      socket.write(bytes.subarray(cut));
      return [answered, await secondReply];
    },
    batch(frames) {
      const replies = frames.map(() => next());
      socket.write(frames.map((frame) => `${JSON.stringify(frame)}\n`).join(""));
      return Promise.all(replies);
    },
    closed: () => within(closed, 30_000, "the bridge to close the connection"),
    close: () => socket.destroy(),
  };
}

export async function refusalOnConnect(path: string): Promise<{ reply: Reply; closed: Promise<void> }> {
  const socket = createConnection(path);
  const reader = new LineReader(1 << 20);
  const closed = new Promise<void>((resolve) => socket.once("close", () => resolve()));
  const reply = new Promise<Reply>((resolve) => {
    socket.on("data", (chunk: Buffer) => {
      const [line] = reader.push(chunk);
      if (line !== undefined) resolve(parse(line));
    });
  });
  socket.on("error", () => socket.destroy());
  return { reply: await within(reply, 30_000, "the bridge's refusal"), closed: within(closed, 30_000, "the close") };
}
