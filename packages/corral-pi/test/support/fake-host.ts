import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual, parseArgs } from "node:util";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../../src/bridge/events.ts";
import { BRIDGE_VERSION, BridgeRefusal, type SubmitResult } from "../../src/bridge/protocol.ts";
import { startBridge } from "../../src/bridge/server.ts";
import { loadSession, saveSession, sessionDir } from "../../src/build/session-store.ts";
import { prepareWorktree } from "../../src/build/worktree.ts";
import { modelSettings, parseRunFlags, sessionSocket } from "../../src/host/config.ts";
import { RemoteRegistry } from "../../src/host/remote/registry.ts";
import { SessionRecord } from "../../src/host/session-record.ts";
import { testHandlers } from "./bridge-client.ts";
import { PACKAGE_ROOT } from "./fixture-install.ts";

type Fields = Record<string, unknown>;
type ScriptEvent = { readonly kind: string; readonly data?: Fields };
type ScriptReply = Fields | readonly ScriptRoute[];
type ScriptRoute = {
  /** First matching route wins; omitted fields are unrestricted. */
  readonly when?: Fields;
  readonly after_call?: string;
  readonly live?: readonly ScriptEvent[];
} & ({ readonly reply: Fields } | { readonly refuse: { readonly code: string; readonly reason: string } });
export interface FakeHostScript {
  /** A Pi recording, relative to this script. Copied unchanged into the session. */
  readonly transcript?: string;
  readonly state?: Fields;
  readonly fleet?: Fields;
  readonly queue_count?: number;
  readonly children_count?: number;
  readonly events?: readonly ScriptEvent[];
  readonly live?: readonly ScriptEvent[];
  /** Absent registered methods are unknown on the real bridge. */
  readonly methods?: Readonly<Record<string, ScriptReply>>;
  readonly builtins?: Partial<Record<"hello" | "get_state" | "answer" | "pause" | "resume" | "cancel" | "rewind"
    | "queue_state", Fields>> & { readonly submit_input?: SubmitResult; readonly queue_withdraw?: ScriptReply; readonly set_model?: ScriptReply };
}

export const FAKE_HOST_ENTRY = fileURLToPath(import.meta.url);
export const FAKE_HOST_SCRIPT_ENV = "CORRAL_PARITY_SCRIPT";
export const shellQuote = (value: string): string => `'${value.replaceAll("'", "'\\''")}'`;

/** The daemon looks up corral-pi on PATH and expects to own the host's process. */
export function writeHostLauncher(directory: string, entry = FAKE_HOST_ENTRY): string {
  const path = join(directory, "corral-pi");
  writeFileSync(path, `#!/bin/sh\nexec ${shellQuote(process.execPath)} --no-env-file ${shellQuote(entry)} "$@"\n`, { mode: 0o700 });
  return path;
}

function launch(argv: readonly string[], env: NodeJS.ProcessEnv, cwd: string) {
  const [command, ...args] = argv;
  if (command === "run" || command === "chat") {
    const config = parseRunFlags(command, args, PACKAGE_ROOT, env, cwd);
    return { ...config.local, cwd, role: config.identity.role };
  }
  if (command !== "build") throw new Error("scripted corral-pi expects run, chat or build");
  const { values } = parseArgs({ args, options: {
    id: { type: "string" }, resume: { type: "string" }, "data-dir": { type: "string" },
    socket: { type: "string" }, headless: { type: "boolean" }, here: { type: "boolean" },
    corral: { type: "string" }, project: { type: "string" }, name: { type: "string" }, profile: { type: "string" },
    model: { type: "string" }, effort: { type: "string" }, fallback: { type: "string", multiple: true },
  } });
  const id = values.resume ?? values.id;
  if (!id || !values["data-dir"]) throw new Error("scripted build requires --id or --resume, and --data-dir");
  const data = resolve(cwd, values["data-dir"]), dir = sessionDir(data, id);
  const socket = values.socket ? resolve(cwd, values.socket) : sessionSocket(dir, env);
  const now = new Date().toISOString();
  const name = values.name?.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40).replace(/-+$/, "") || id.slice(2);
  const here = values.here === true;
  const record = values.resume ? loadSession(data, id) : {
    version: 1 as const, id, name, created: now, updated: now, repoRoot: cwd, here,
    ...prepareWorktree(cwd, name, here), workspaceRoot: env.CORRAL_NEXT_ROOT, project: values.project,
    route: modelSettings(values),
  };
  return { dir, id, socket, cwd: record.worktree, role: "build", headless: values.headless || !process.stdin.isTTY,
    saveBuild: (sessionFile: string) => saveSession(data, { ...record, pid: process.pid, socket, sessionFile, updated: now }) };
}

function appendEvents(log: EventLog, events: readonly ScriptEvent[] = []): void {
  for (const event of events) log.emit(event.kind, event.data);
}

function routed(script: ScriptReply, args: Fields, completed: ReadonlySet<string>, live: EventLog): Fields {
  if (!Array.isArray(script)) return script as Fields;
  const route = (script as readonly ScriptRoute[]).find(route => (!route.after_call || completed.has(route.after_call))
    && Object.entries(route.when ?? {}).every(([key, value]) => isDeepStrictEqual(args[key], value)));
  if (!route) throw new BridgeRefusal("bad_input", "no scripted reply for these arguments");
  if ("refuse" in route) throw new BridgeRefusal(route.refuse.code, route.refuse.reason);
  appendEvents(live, route.live);
  return route.reply;
}

/** Run the real socket protocol against a frozen scenario. The script never runs tools. */
export async function startFakeHost(scriptPath: string, argv: readonly string[], env = process.env, cwd = process.cwd()) {
  const scriptFile = resolve(scriptPath);
  const source = readFileSync(scriptFile, "utf8");
  const host = launch(argv, env, cwd);
  const script = JSON.parse(source, (_key, value: unknown) =>
    typeof value === "string" && (value === "$worktree" || value.startsWith("$worktree/"))
      ? host.cwd + value.slice("$worktree".length) : value) as FakeHostScript;
  const sessions = join(host.dir, "sessions");
  mkdirSync(host.dir, { recursive: true, mode: 0o700 });
  const record = new SessionRecord(host.dir, host.id, host.role, host.cwd, false, host.socket);
  let sessionFile = record.transcript;
  if (script.transcript !== undefined) {
    mkdirSync(sessions, { recursive: true, mode: 0o700 });
    sessionFile = join(sessions, "recorded.jsonl");
    copyFileSync(resolve(dirname(scriptFile), script.transcript), sessionFile);
  } else if (!sessionFile || !existsSync(sessionFile)) {
    const session = SessionManager.create(host.cwd, sessions);
    session.appendCustomEntry("corral_host_started", { pid: process.pid }, { immediate: true });
    sessionFile = session.getSessionFile();
  }
  if (!sessionFile) throw new Error("scripted host has no Pi transcript");
  const transcript = sessionFile;
  const events = new EventLog(), live = new EventLog();
  const bridge = await startBridge({ path: host.socket, sessionId: host.id, events });
  try {
    const model = "openai-codex/gpt-6.1-sol";
    const queue = Array.from({ length: script.queue_count ?? 0 }, (_, index) => {
      const text = `Queued follow-up ${index + 1}`;
      return { key: `input-${index + 1}`, policy: "follow_up", origin: "owner", text, bytes: Buffer.byteLength(text), actions: [] };
    });
    const children = Array.from({ length: script.children_count ?? 0 }, (_, index) => ({ id: `child-${index + 1}`,
      kind: "slice", title: `Child ${index + 1}`, state: index < 3 ? "running" : "done", model, tokens: index * 20 }));
    const fleet = script.fleet ?? (children.length ? { children, messages: [], messages_total: 0, max_parallel: 3 } : undefined);
    const state = { mode: host.headless ? "headless" : "interactive", turn: 0, open: false, paused: false, model,
      last_error: undefined as string | undefined,
      usage: { tokens: 0, cost_partial: false, context_tokens: 0, context_window: 0 },
      queue: queue.slice(0, 12), queue_total: queue.length, children: { live: Math.min(3, children.length), waiting: 0, total: children.length },
      jobs: { live: 0, waiting: 0, total: 0 }, ...script.state };
    appendEvents(events, script.events);
    appendEvents(live, script.live);
    const fleetSeq = fleet ? live.emit("live_fleet").seq : 0;
    const stateSeq = live.emit("live_state").seq;
    const projectionBytes = Buffer.byteLength(JSON.stringify({ state, fleet }));
    const completed = new Set<string>();
    const dispatch = (name: string, result: ScriptReply, args: Fields) => {
      const reply = routed(result, args, completed, live);
      completed.add(name);
      return reply;
    };
    const registry = new RemoteRegistry();
    for (const [name, result] of Object.entries(script.methods ?? {})) {
      registry.register({ name, parse: args => args, run: args => dispatch(name, result, args) });
    }
    const replies = script.builtins;
    if ("saveBuild" in host) host.saveBuild?.(transcript);
    record.save(transcript, state.paused);
    bridge.attach(testHandlers({
      hello: () => ({ ...replies?.hello, protocol: BRIDGE_VERSION, role: host.role, session_id: host.id,
        session_file: transcript, pid: process.pid }),
      state: () => ({ mode: state.mode, turn: { current: state.turn, open: state.open }, paused: state.paused,
        owner_questions: [], last_error: state.last_error,
        ...replies?.get_state }),
      submit: async () => replies?.submit_input ?? { disposition: "rejected", reason: "the scripted host is read-only" },
      answer: async () => replies?.answer ?? { status: "missing" },
      pause: async () => replies?.pause ?? { paused: state.paused },
      resume: async () => replies?.resume ?? { paused: state.paused },
      cancel: async () => replies?.cancel ?? { cancelled: false },
      rewind: async () => replies?.rewind ?? { checkpoints: [], rewinds: [] },
      queueState: limit => replies?.queue_state ?? { available: queue.length > 0,
        items: queue.slice(0, limit).map((item, index) => ({ ...item, item: item.key, mode: item.policy, state: "queued", position: index + 1 })),
        more: queue.length > limit },
      queueWithdraw: key => replies?.queue_withdraw
        ? dispatch("queue_withdraw", replies.queue_withdraw, { key }) : { outcome: "unknown_item" },
      setModel: async args => replies?.set_model
        ? dispatch("set_model", replies.set_model, args) : { model: state.model },
      shutdown: () => { void bridge.close(); },
    }), {
      since(seq, byteLimit) {
        const page = live.since(seq, Math.min(byteLimit, 56 * 1024) - projectionBytes - 256, 32);
        return { ...page, ...(seq < stateSeq || page.gap ? { state } : {}),
          ...(fleet && (seq < fleetSeq || page.gap) ? { fleet } : {}) };
      },
    }, registry);
    return bridge;
  } catch (error) {
    await bridge.close();
    throw error;
  }
}

if (import.meta.main) {
  try {
    const script = process.env[FAKE_HOST_SCRIPT_ENV];
    if (!script) throw new Error(`set ${FAKE_HOST_SCRIPT_ENV} to a state script`);
    const host = await startFakeHost(script, process.argv.slice(2));
    const stop = () => { void host.close(); };
    process.once("SIGINT", stop);
    process.once("SIGTERM", stop);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 2;
  }
}
