// The host's bridge server: a Unix socket in the generation's 0700 runtime directory. Every
// connection's peer uid must be the host's; every request must carry the generation's ctl_cap
// and name the current generation. Refused connections are closed.
import { timingSafeEqual } from "node:crypto";
import { chmodSync, existsSync, unlinkSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";

import type { PeerLookup } from "../host/peer.ts";
import { canonicalJson, sha256Hex } from "../tools/corral.ts";
import type { EventLog } from "./events.ts";
import {
  BRIDGE_VERSION, BridgeRefusal, FRAME_MAX_BYTES, MAX_IN_FLIGHT, type RequestFrame, type SubmitInput,
  type SubmitResult, frameId, parseFrame, parseSeq, parseSubmitInput, parseTurn,
} from "./protocol.ts";

export interface BridgeHandlers {
  hello(): Record<string, unknown>;
  state(): Record<string, unknown>;
  submit(input: SubmitInput, requestId: string): Promise<SubmitResult>;
  cancel(turn: number | undefined): Promise<Record<string, unknown>>;
  shutdown(): void;
}

export interface BridgeServerOptions {
  readonly path: string;
  readonly ctlCap: string;
  readonly generation: number;
  readonly uid: number;
  readonly peer: PeerLookup;
  readonly events: EventLog;
  readonly handlers: BridgeHandlers;
}

interface Submission {
  readonly digest: string;
  readonly result: Promise<SubmitResult>;
}

const SUBMISSIONS_MAX = 256;

export interface BridgeServer {
  readonly path: string;
  close(): Promise<void>;
}

function capMatches(expected: Buffer, presented: string): boolean {
  const actual = Buffer.from(presented, "utf8");
  return actual.length === expected.length && timingSafeEqual(actual, expected);
}

export async function startBridge(options: BridgeServerOptions): Promise<BridgeServer> {
  const expected = Buffer.from(options.ctlCap, "utf8");
  const submissions = new Map<string, Submission>();
  let inputs: Promise<unknown> = Promise.resolve();
  const sockets = new Set<Socket>();

  const reply = (socket: Socket, id: string | null, body: Record<string, unknown>) => {
    if (!socket.destroyed) {
      socket.write(`${JSON.stringify({ version: BRIDGE_VERSION, id, generation: options.generation, ...body })}\n`);
    }
  };
  const refuse = (socket: Socket, id: string | null, refusal: BridgeRefusal, close: boolean) => {
    reply(socket, id, { ok: false, error: { code: refusal.code, reason: refusal.reason } });
    if (close) socket.end();
  };

  const submit = (input: SubmitInput, requestId: string): Promise<SubmitResult & { replayed?: boolean }> => {
    const digest = sha256Hex(canonicalJson({ text: input.text, busy_policy: input.busyPolicy, origin: input.origin }));
    const prior = submissions.get(input.idempotencyKey);
    if (prior) {
      if (prior.digest !== digest) {
        return Promise.resolve({ disposition: "rejected", reason: "idempotency_conflict" });
      }
      return prior.result.then((result) => ({ ...result, replayed: true }));
    }
    const result = inputs.then(() => options.handlers.submit(input, requestId));
    inputs = result.catch(() => undefined);
    submissions.set(input.idempotencyKey, { digest, result });
    if (submissions.size > SUBMISSIONS_MAX) {
      const oldest = submissions.keys().next().value;
      if (oldest !== undefined) submissions.delete(oldest);
    }
    return result;
  };

  const run = async (frame: RequestFrame): Promise<Record<string, unknown>> => {
    switch (frame.command) {
      case "hello":
        return options.handlers.hello();
      case "get_state":
        return options.handlers.state();
      case "submit_input": {
        const input = parseSubmitInput(frame.args);
        const result = await submit(input, frame.id);
        options.events.emit("input", {
          request_id: frame.id, idempotency_key: input.idempotencyKey, origin: input.origin,
          busy_policy: input.busyPolicy, disposition: result.disposition, ...(result.turn ? { turn: result.turn } : {}),
        });
        return { request_id: frame.id, idempotency_key: input.idempotencyKey, ...result };
      }
      case "cancel":
        return { request_id: frame.id, ...(await options.handlers.cancel(parseTurn(frame.args))) };
      case "events_since":
        return { ...options.events.since(parseSeq(frame.args)) };
      case "shutdown":
        setImmediate(() => options.handlers.shutdown());
        return { shutting_down: true };
    }
  };

  const handle = async (socket: Socket, line: string) => {
    let value: unknown;
    try {
      value = JSON.parse(line);
    } catch {
      refuse(socket, null, new BridgeRefusal("bad_frame", "a request is one JSON object"), true);
      return;
    }
    const id = frameId(value);
    let frame: RequestFrame;
    try {
      frame = parseFrame(value);
    } catch (error) {
      refuse(socket, id, error instanceof BridgeRefusal ? error : new BridgeRefusal("bad_frame", "unreadable"), true);
      return;
    }
    if (!capMatches(expected, frame.ctlCap)) {
      refuse(socket, frame.id, new BridgeRefusal("auth_failed", "ctl_cap does not match this generation"), true);
      return;
    }
    if (frame.generation !== options.generation) {
      refuse(socket, frame.id, new BridgeRefusal("stale_generation",
        `this host serves generation ${options.generation}`), false);
      return;
    }
    try {
      reply(socket, frame.id, { ok: true, result: await run(frame) });
    } catch (error) {
      const refusal = error instanceof BridgeRefusal ? error
        : new BridgeRefusal("host_error", error instanceof Error ? error.message.slice(0, 512) : "failed");
      refuse(socket, frame.id, refusal, false);
    }
  };

  const server: Server = createServer((socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    socket.on("error", () => socket.destroy());
    const peer = options.peer(socket);
    if (!peer || peer.uid !== options.uid) {
      refuse(socket, null, new BridgeRefusal("peer_refused",
        peer ? `peer uid ${peer.uid} is not the host's uid` : "peer credentials are unavailable"), true);
      return;
    }
    let buffer = "";
    let inFlight = 0;
    socket.on("data", (chunk: Buffer) => {
      buffer += chunk.toString("utf8");
      let boundary: number;
      while ((boundary = buffer.indexOf("\n")) !== -1) {
        const line = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 1);
        if (Buffer.byteLength(line) > FRAME_MAX_BYTES) {
          refuse(socket, null, new BridgeRefusal("frame_too_large", `frames are at most ${FRAME_MAX_BYTES} bytes`), true);
          return;
        }
        if (inFlight >= MAX_IN_FLIGHT) {
          refuse(socket, frameId(safeParse(line)), new BridgeRefusal("busy", `at most ${MAX_IN_FLIGHT} requests in flight`), false);
          continue;
        }
        inFlight++;
        void handle(socket, line).finally(() => inFlight--);
      }
      if (Buffer.byteLength(buffer) > FRAME_MAX_BYTES) {
        refuse(socket, null, new BridgeRefusal("frame_too_large", `frames are at most ${FRAME_MAX_BYTES} bytes`), true);
      }
    });
  });

  if (existsSync(options.path)) unlinkSync(options.path);
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(options.path, () => {
      chmodSync(options.path, 0o600);
      resolve();
    });
  });

  return {
    path: options.path,
    close: () => new Promise<void>((resolve) => {
      for (const socket of sockets) socket.destroy();
      server.close(() => resolve());
    }),
  };
}

function safeParse(line: string): unknown {
  try {
    return JSON.parse(line);
  } catch {
    return undefined;
  }
}
