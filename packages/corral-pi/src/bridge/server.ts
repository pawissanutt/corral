import { chmodSync, lstatSync, mkdirSync, unlinkSync } from "node:fs";
import { createConnection, createServer, type Socket } from "node:net";
import { dirname } from "node:path";

import type { EventLog } from "./events.ts";
import { FrameInvalid, FrameTooLarge, LineReader } from "./framing.ts";
import {
  BRIDGE_VERSION, BridgeRefusal, EVENT_PAGE_BYTES, FRAME_MAX_BYTES, INPUT_REPLY_MS, MAX_IN_FLIGHT,
  type AnswerInput, type RequestFrame, type RewindInput, type SessionInput, type SubmitResult,
  frameId, parseAnswer, parseFrame, parseQueueLimit, parseQueueWithdraw, parseRewind, parseSeq, parseSubmitInput, parseTurn,
} from "./protocol.ts";

export interface BridgeHandlers {
  hello(): Record<string, unknown>;
  state(): Record<string, unknown>;
  submit(input: SessionInput, requestId: string): Promise<SubmitResult>;
  answer(input: AnswerInput): Promise<Record<string, unknown>>;
  pause(): Promise<Record<string, unknown>>;
  resume(): Promise<Record<string, unknown>>;
  cancel(turn: number | undefined): Promise<Record<string, unknown>>;
  rewind(input: RewindInput): Promise<Record<string, unknown>>;
  shutdown(): void;
  queueState(limit: number): Record<string, unknown>;
  queueWithdraw(key: string): Record<string, unknown>;
  setModel(args: Record<string, unknown>): Promise<Record<string, unknown>>;
}

interface EventRing {
  since(seq: number, byteLimit: number): object;
}

export interface BridgeServerOptions {
  readonly path: string;
  readonly sessionId: string;
  readonly events: EventLog;
  readonly handlers?: BridgeHandlers;
  readonly liveEvents?: EventRing;
  readonly inputReplyMs?: number;
}

export interface BridgeServer {
  readonly path: string;
  /** Reserve the socket before opening the transcript, then answer requests once ready. */
  attach(handlers: BridgeHandlers, liveEvents: EventRing): void;
  close(): Promise<void>;
}

function refusal(error: unknown): BridgeRefusal {
  return error instanceof BridgeRefusal ? error
    : new BridgeRefusal("host_error", error instanceof Error ? error.message.slice(0, 512) : "failed");
}

function safeParse(line: string): unknown {
  try { return JSON.parse(line); } catch { return undefined; }
}

function socketStat(path: string) {
  try { return lstatSync(path); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    return undefined;
  }
}

function canConnect(path: string): Promise<boolean> {
  const { promise, resolve, reject } = Promise.withResolvers<boolean>();
  const socket = createConnection(path);
  socket.once("connect", () => { socket.destroy(); resolve(true); });
  socket.once("error", (error: NodeJS.ErrnoException) => {
    if (error.code === "ECONNREFUSED" || error.code === "ENOENT") resolve(false);
    else reject(error);
  });
  return promise;
}

export function preparePrivateDirectory(path: string): void {
  mkdirSync(path, { recursive: true, mode: 0o700 });
  const directory = lstatSync(path);
  if (!directory.isDirectory() || directory.uid !== process.getuid?.() || (directory.mode & 0o777) !== 0o700) {
    throw new Error(`session socket directory must be owned by this user and have mode 0700: ${path}`);
  }
}

async function prepareSocket(path: string, liveError: Error): Promise<void> {
  preparePrivateDirectory(dirname(path));
  const uid = process.getuid?.();
  const old = socketStat(path);
  if (!old) return;
  if (!old.isSocket() || old.uid !== uid) throw new Error(`not an owned Unix socket: ${path}`);
  // A live listener may still be starting up; connection alone is enough to leave it intact.
  if (await canConnect(path)) throw liveError;
  const current = socketStat(path);
  if (!current) return;
  if (current.ino !== old.ino || current.dev !== old.dev) throw liveError;
  unlinkSync(path);
}

export async function startBridge(options: BridgeServerOptions): Promise<BridgeServer> {
  const liveError = new Error(`session already live; corral stop ${options.sessionId}`);
  await prepareSocket(options.path, liveError);
  let handlers = options.handlers;
  let liveEvents = options.liveEvents;
  const sockets = new Set<Socket>();

  const reply = (socket: Socket, id: string | null, body: Record<string, unknown>) => {
    if (socket.destroyed) return;
    const head = { version: BRIDGE_VERSION, id };
    let text = JSON.stringify({ ...head, ...body });
    if (Buffer.byteLength(text) + 1 > FRAME_MAX_BYTES) {
      text = JSON.stringify({ ...head, ok: false, error: { code: "response_too_large",
        reason: `frames are at most ${FRAME_MAX_BYTES} bytes` } });
    }
    socket.write(`${text}\n`);
  };
  const refuse = (socket: Socket, id: string | null, { code, reason }: BridgeRefusal, close = false) => {
    reply(socket, id, { ok: false, error: { code, reason } });
    if (close) socket.end();
  };

  const submit = async (host: BridgeHandlers, frame: RequestFrame): Promise<Record<string, unknown>> => {
    const input = parseSubmitInput(frame.args);
    const request = { request_id: frame.id, origin: input.origin, busy_policy: input.busyPolicy };
    const outcome = host.submit(input, frame.id);
    const pending = Symbol("pending");
    const deadline = Promise.withResolvers<typeof pending>();
    const timer = setTimeout(() => deadline.resolve(pending), options.inputReplyMs ?? INPUT_REPLY_MS);
    const result = await Promise.race([outcome, deadline.promise]).finally(() => clearTimeout(timer));
    if (result === pending) {
      options.events.emit("input", { ...request, disposition: "pending" });
      void outcome.then(
        settled => options.events.emit("input_settled", { ...request, ...settled }),
        error => {
          const { code, reason } = refusal(error);
          options.events.emit("input_settled", { ...request, error: { code, reason } });
        },
      );
      return { request_id: frame.id, disposition: "pending", reason: "preflight_pending" };
    }
    options.events.emit("input", { ...request, ...result });
    return { request_id: frame.id, ...result };
  };

  const run = async (frame: RequestFrame): Promise<Record<string, unknown>> => {
    const host = handlers;
    if (!host) throw new BridgeRefusal("starting", "the session is starting");
    switch (frame.command) {
      case "hello": return host.hello();
      case "get_state": return host.state();
      case "submit_input": return submit(host, frame);
      case "answer": return host.answer(parseAnswer(frame.args));
      case "pause": return host.pause();
      case "resume": return host.resume();
      case "cancel": return { request_id: frame.id, ...(await host.cancel(parseTurn(frame.args))) };
      case "rewind": return host.rewind(parseRewind(frame.args));
      case "queue_state": return host.queueState(parseQueueLimit(frame.args));
      case "queue_withdraw": return host.queueWithdraw(parseQueueWithdraw(frame.args));
      case "set_model": return host.setModel(frame.args);
      case "events_since": return { ...options.events.since(parseSeq(frame.args), EVENT_PAGE_BYTES) };
      case "live_since": {
        const seq = parseSeq(frame.args);
        if (!liveEvents) throw new BridgeRefusal("live_unavailable", "the session has no live event stream");
        return { ...liveEvents.since(seq, EVENT_PAGE_BYTES) };
      }
      case "shutdown":
        setImmediate(() => host.shutdown());
        return { shutting_down: true };
    }
  };

  const handle = async (socket: Socket, line: string) => {
    const value = safeParse(line);
    let frame: RequestFrame;
    try { frame = parseFrame(value); }
    catch (error) { refuse(socket, frameId(value), refusal(error), true); return; }
    try { reply(socket, frame.id, { ok: true, result: await run(frame) }); }
    catch (error) { refuse(socket, frame.id, refusal(error)); }
  };

  const server = createServer(socket => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    socket.on("error", () => socket.destroy());
    const reader = new LineReader(FRAME_MAX_BYTES);
    let inFlight = 0;
    socket.on("data", (chunk: Buffer) => {
      let lines: string[];
      try { lines = reader.push(chunk); }
      catch (error) {
        const code = error instanceof FrameTooLarge || error instanceof FrameInvalid ? error.code : "bad_frame";
        refuse(socket, null, new BridgeRefusal(code, error instanceof Error ? error.message : "unreadable"), true);
        socket.removeAllListeners("data");
        return;
      }
      for (const line of lines) {
        if (inFlight >= MAX_IN_FLIGHT) {
          refuse(socket, frameId(safeParse(line)), new BridgeRefusal("busy", `at most ${MAX_IN_FLIGHT} requests in flight`));
          continue;
        }
        inFlight++;
        void handle(socket, line).finally(() => inFlight--);
      }
    });
  });

  const listening = Promise.withResolvers<void>();
  server.once("error", (error: NodeJS.ErrnoException) => listening.reject(error.code === "EADDRINUSE" ? liveError : error));
  server.listen(options.path, () => {
    try { chmodSync(options.path, 0o600); listening.resolve(); }
    catch (error) { server.close(); listening.reject(error); }
  });
  await listening.promise;

  let closed: Promise<void> | undefined;
  return {
    path: options.path,
    attach(next, live) { handlers = next; liveEvents = live; },
    close() {
      if (closed) return closed;
      const { promise, resolve } = Promise.withResolvers<void>();
      closed = promise;
      for (const socket of sockets) socket.destroy();
      server.close(() => resolve());
      return closed;
    },
  };
}
