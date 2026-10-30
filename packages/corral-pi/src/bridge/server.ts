// The host's bridge server: a Unix socket in the generation's 0700 runtime directory. Every
// connection's peer uid must be the host's; every request must carry the generation's ctl_cap
// and name the current generation. Refused connections are closed.
import { timingSafeEqual } from "node:crypto";
import { chmodSync, existsSync, unlinkSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";

import type { PeerLookup } from "../host/peer.ts";
import { canonicalJson, sha256Hex } from "../tools/corral.ts";
import type { EventLog } from "./events.ts";
import { FrameInvalid, FrameTooLarge, LineReader } from "./framing.ts";
import {
  BRIDGE_VERSION, BridgeRefusal, EVENT_PAGE_BYTES, FRAME_MAX_BYTES, INPUT_REPLY_MS, MAX_IN_FLIGHT, type QuestionUpdate,
  type RequestFrame, SUBMISSIONS_MAX, type SubmitInput, type SubmitResult, frameId, parseFrame, parseQuestions, parseSeq,
  parseSubmitInput, parseTurn,
} from "./protocol.ts";

export interface BridgeHandlers {
  hello(): Record<string, unknown>;
  state(): Record<string, unknown>;
  /** Resolves with upstream's own disposition, however long upstream takes to give it. */
  submit(input: SubmitInput, requestId: string): Promise<SubmitResult>;
  cancel(turn: number | undefined): Promise<Record<string, unknown>>;
  shutdown(): void;
  /** Corral's current states of the container's questions; absent, the host keeps no projection. */
  questions?(updates: readonly QuestionUpdate[]): Record<string, unknown>;
}

export interface BridgeServerOptions {
  readonly path: string;
  readonly ctlCap: string;
  readonly generation: number;
  readonly uid: number;
  readonly peer: PeerLookup;
  readonly events: EventLog;
  readonly handlers: BridgeHandlers;
  readonly submissionsMax?: number;
  readonly inputReplyMs?: number;
}

/** One admitted input key. It is never forgotten while the generation lives. */
interface Submission {
  readonly digest: string;
  readonly requestId: string;
  readonly outcome: Promise<SubmitResult>;
  settled?: SubmitResult;
  failed?: BridgeRefusal;
  reportedPending: boolean;
}

export interface BridgeServer {
  readonly path: string;
  close(): Promise<void>;
}

const PENDING: unique symbol = Symbol("pending");

function capMatches(expected: Buffer, presented: string): boolean {
  const actual = Buffer.from(presented, "utf8");
  return actual.length === expected.length && timingSafeEqual(actual, expected);
}

export async function startBridge(options: BridgeServerOptions): Promise<BridgeServer> {
  const expected = Buffer.from(options.ctlCap, "utf8");
  const capacity = options.submissionsMax ?? SUBMISSIONS_MAX;
  const replyMs = options.inputReplyMs ?? INPUT_REPLY_MS;
  const submissions = new Map<string, Submission>();
  let inputs: Promise<unknown> = Promise.resolve();
  const sockets = new Set<Socket>();

  const reply = (socket: Socket, id: string | null, body: Record<string, unknown>) => {
    if (socket.destroyed) return;
    const head = { version: BRIDGE_VERSION, id, generation: options.generation };
    let text = JSON.stringify({ ...head, ...body });
    const size = Buffer.byteLength(text) + 1;
    if (size > FRAME_MAX_BYTES) {
      text = JSON.stringify({ ...head, ok: false, error: { code: "response_too_large",
        reason: `the reply would be ${size} bytes; frames are at most ${FRAME_MAX_BYTES}` } });
    }
    socket.write(`${text}\n`);
  };
  const refuse = (socket: Socket, id: string | null, refusal: BridgeRefusal, close: boolean) => {
    reply(socket, id, { ok: false, error: { code: refusal.code, reason: refusal.reason } });
    if (close) socket.end();
  };

  const answer = async (entry: Submission, replayed: boolean): Promise<SubmitResult & { replayed?: true }> => {
    const mark = replayed ? { replayed: true as const } : {};
    if (entry.failed) throw entry.failed;
    if (entry.settled) return { ...entry.settled, ...mark };
    let timer: ReturnType<typeof setTimeout> | undefined;
    const outcome = await Promise.race([
      entry.outcome,
      new Promise<typeof PENDING>((resolve) => { timer = setTimeout(() => resolve(PENDING), replyMs); }),
    ]).finally(() => clearTimeout(timer));
    if (outcome !== PENDING) return { ...outcome, ...mark };
    entry.reportedPending = true;
    return { disposition: "pending", reason: "preflight_pending", ...mark };
  };

  const admit = (input: SubmitInput, requestId: string, digest: string): Submission => {
    const outcome = inputs.then(() => options.handlers.submit(input, requestId));
    const entry: Submission = { digest, requestId, outcome, reportedPending: false };
    outcome.then(
      (result) => {
        entry.settled = result;
        if (entry.reportedPending) {
          options.events.emit("input_settled", {
            request_id: requestId, idempotency_key: input.idempotencyKey, ...result,
          });
        }
      },
      (error: unknown) => {
        entry.failed = new BridgeRefusal("host_error", error instanceof Error ? error.message.slice(0, 512) : "failed");
        if (entry.reportedPending) {
          options.events.emit("input_settled", {
            request_id: requestId, idempotency_key: input.idempotencyKey, error: entry.failed.code,
          });
        }
      },
    );
    submissions.set(input.idempotencyKey, entry);
    return entry;
  };

  const submit = async (input: SubmitInput, requestId: string): Promise<SubmitResult & { replayed?: true }> => {
    const digest = sha256Hex(canonicalJson({ text: input.text, busy_policy: input.busyPolicy, origin: input.origin }));
    const prior = submissions.get(input.idempotencyKey);
    if (prior) {
      if (prior.digest !== digest) return { disposition: "rejected", reason: "idempotency_conflict" };
      return answer(prior, true);
    }
    if (submissions.size >= capacity) {
      throw new BridgeRefusal("input_limit",
        `generation ${options.generation} admitted ${capacity} input keys; replays still answer, new keys need a new generation`);
    }
    const entry = admit(input, requestId, digest);
    const first = answer(entry, false);
    inputs = first.then(() => undefined, () => undefined);
    return first;
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
        return { ...options.events.since(parseSeq(frame.args), EVENT_PAGE_BYTES) };
      case "shutdown":
        setImmediate(() => options.handlers.shutdown());
        return { shutting_down: true };
      case "questions": {
        const updates = parseQuestions(frame.args);
        return options.handlers.questions?.(updates) ?? { applied: false };
      }
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
    const reader = new LineReader(FRAME_MAX_BYTES);
    let inFlight = 0;
    socket.on("data", (chunk: Buffer) => {
      let lines: string[];
      try {
        lines = reader.push(chunk);
      } catch (error) {
        const code = error instanceof FrameTooLarge || error instanceof FrameInvalid ? error.code : "bad_frame";
        refuse(socket, null, new BridgeRefusal(code, error instanceof Error ? error.message : "unreadable"), true);
        socket.removeAllListeners("data");
        return;
      }
      for (const line of lines) {
        if (inFlight >= MAX_IN_FLIGHT) {
          refuse(socket, frameId(safeParse(line)), new BridgeRefusal("busy", `at most ${MAX_IN_FLIGHT} requests in flight`), false);
          continue;
        }
        inFlight++;
        void handle(socket, line).finally(() => inFlight--);
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
