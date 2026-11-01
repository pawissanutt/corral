// A real HostSession with Code Mode enabled for one role, a receipt-keeping fake corral, scripted
// fixture-provider turns and in-process fixture tools whose completion the test controls
// (`fixture_hold`), so every interleaving is driven by barriers, never by timing.
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ExtensionUIContext, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { type BridgeEvent, EventLog } from "../../src/bridge/events.ts";
import { loadModules } from "../../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../src/host/restrict.ts";
import { HostSession, authContext } from "../../src/host/session.ts";
import type { CapabilitySet, HostIdentity, RoleContext } from "../../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../../src/tools/corral.ts";
import { fixtureTransport } from "../fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./fixture-install.ts";
import { type ReceiptCorral, writeReceiptCorral } from "./receipt-corral.ts";
import { nextEvent } from "./wait.ts";

export type CodeRole = "worker" | "pm" | "assistant";

export const IDENTITIES: Record<CodeRole, HostIdentity> = {
  worker: { store: "st-demo", project: "demo", role: "worker", generation: 1, task: "T-683", run: "R1" },
  pm: { store: "st-demo", project: "demo", role: "pm", generation: 1 },
  assistant: { store: "st-demo", project: "", role: "assistant", assistant: "A-1", generation: 1 },
};

interface Gate {
  readonly started: Promise<void>;
  release(text: string): void;
}

/** Each `fixture_hold({name})` call waits until the test releases that name. */
class Holds {
  private readonly gates = new Map<string, { started: () => void; release: (text: string) => void; at: Promise<void>; done: Promise<string> }>();
  runs = 0;

  private gate(name: string) {
    let entry = this.gates.get(name);
    if (!entry) {
      let started!: () => void;
      let release!: (text: string) => void;
      const at = new Promise<void>((resolve) => { started = resolve; });
      const done = new Promise<string>((resolve) => { release = resolve; });
      entry = { started, release, at, done };
      this.gates.set(name, entry);
    }
    return entry;
  }

  wait(name: string): Promise<string> {
    this.runs++;
    const entry = this.gate(name);
    entry.started();
    return entry.done;
  }

  get(name: string): Gate {
    const entry = this.gate(name);
    return { started: entry.at, release: entry.release };
  }
}

export interface Confirmations {
  readonly asked: string[];
  /** Resolves the oldest pending owner confirmation. */
  answer(yes: boolean): void;
  readonly pending: () => number;
  next(): Promise<void>;
}

export interface CodeFixture {
  readonly host: HostSession;
  readonly events: EventLog;
  readonly corral: ReceiptCorral;
  readonly cwd: string;
  readonly holds: Holds;
  readonly confirmations: Confirmations;
  readonly big: string;
  forbiddenRuns(): number;
  /** Scripts the provider and submits one prompt; resolves after its turn ended and reported. */
  turn(steps: unknown[], text?: string): Promise<void>;
  /** The model-visible description of the codemode tool as last prepared. */
  description(): string;
  /** The model-visible result of the codemode call whose scripted ID is `callId`. */
  result(callId: string): { text: string; isError: boolean; details: unknown };
  journal(): Record<string, unknown>[];
  since(from: number): readonly BridgeEvent[];
  close(): Promise<void>;
}

export interface CodeFixtureOptions {
  readonly enabled?: readonly string[];
  readonly bigBytes?: number;
}

export async function codeHost(role: CodeRole, options: CodeFixtureOptions = {}): Promise<CodeFixture> {
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), `corral-code-${role}-`));
  const originalFetch = globalThis.fetch;
  for (const tree of [`src/roles/${role}`, "src/roles/common", "src/extensions", "assets"]) {
    cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
  }
  const cwd = join(work, "cwd");
  mkdirSync(join(cwd, ".corral"), { recursive: true });
  for (const name of ["one.txt", "two.txt"]) {
    writeFileSync(join(cwd, name), `${"discard this line\n".repeat(50)}keep: ${name}\n`);
  }
  restrictEnvironment(process.env);
  const identity = IDENTITIES[role];
  const modules = await loadModules(assets.dir, role);
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("fixture transport missing");
  const network = guardedFetch(transport);
  globalThis.fetch = network.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const events = new EventLog(1);
  const corral = writeReceiptCorral(work);
  const holds = new Holds();
  const big = "x".repeat(options.bigBytes ?? 400 * 1024);
  let forbiddenRuns = 0;
  const local: ToolDefinition[] = [
    {
      name: "fixture_hold", label: "Hold", description: "Waits until the test releases the named gate.",
      parameters: Type.Object({ name: Type.String() }),
      async execute(_id, params) {
        return { content: [{ type: "text", text: await holds.wait((params as { name: string }).name) }], details: undefined };
      },
    },
    {
      name: "fixture_big", label: "Big", description: "Returns a large text.", parameters: Type.Object({}),
      async execute() {
        return { content: [{ type: "text", text: `${big}\nneedle: the selected line\n${big}` }], details: undefined };
      },
    },
  ];
  // Registered and callable upstream, but not in the role's loadout: the host must refuse it.
  const forbidden: ToolDefinition = {
    name: "fixture_forbidden", label: "Forbidden", description: "Outside the role loadout", exposure: "codemode",
    parameters: Type.Object({}),
    async execute() {
      forbiddenRuns++;
      return { content: [{ type: "text", text: "must never run" }], details: undefined };
    },
  };
  const base = modules.role;
  const wrapped = { ...modules, role: { ...base,
    localToolNames: [...(base.localToolNames ?? []), ...local.map((tool) => tool.name)],
    localTools: (context: RoleContext) => [...(base.localTools?.(context) ?? []), ...local],
    extend(pi: Parameters<NonNullable<typeof base.extend>>[0], context: RoleContext) {
      base.extend?.(pi, context);
      pi.registerTool(forbidden);
    },
  } };
  const capabilities: CapabilitySet = { catalog: 4, digest: "a".repeat(64), preset: role === "pm" ? "full" : "standard",
    enabled: [...(options.enabled ?? ["codemode"])] };
  let host: HostSession | undefined;
  const client = new CorralClient({ corral: corral.path, root: work, identity, hostCap: "h".repeat(64),
    sessionId: () => host?.sessionId() ?? "" });
  const live = await HostSession.create({
    // A late/foreign extension makes the forbidden tool callable upstream; only host policy refuses it.
    sdk: { ...sdk, createAgentSessionFromServices(opts) {
      return sdk.createAgentSessionFromServices({ ...opts, tools: [...opts.tools ?? [], forbidden.name] });
    } }, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", capabilities, ui: "plain",
    guidance: { projectText: false, settings: {} as never }, modules: wrapped, events, network, client,
    sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
  host = live;
  const asked: string[] = [];
  const pending: ((yes: boolean) => void)[] = [];
  const waiters: (() => void)[] = [];
  const ui = { confirm: (_title: string, message: string) => new Promise<boolean>((resolve) => {
    asked.push(message);
    pending.push(resolve);
    for (const waiter of waiters.splice(0)) waiter();
  }) };
  await live.session.bindExtensions({ uiContext: ui as unknown as ExtensionUIContext });
  await live.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
  let seq = 0;
  return {
    host: live, events, corral, cwd, holds, big,
    confirmations: {
      asked, answer: (yes) => pending.shift()?.(yes), pending: () => pending.length,
      next: () => pending.length > 0 ? Promise.resolve() : new Promise<void>((resolve) => waiters.push(resolve)),
    },
    forbiddenRuns: () => forbiddenRuns,
    async turn(steps, text = "go") {
      fixtureTransport().script(steps as never);
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      const key = `turn-${++seq}`;
      const submitted = await live.submit({ text, busyPolicy: "prompt", idempotencyKey: key, origin: "test" }, `rq-${key}`);
      if (submitted.disposition !== "started") throw new Error(`turn not started: ${JSON.stringify(submitted)}`);
      await ended;
      await live.drained();
    },
    description: () => live.session.agent.state.tools.find((tool) => tool.name === "codemode")?.description ?? "",
    result(callId) {
      const message = live.session.messages.find((m) => m.role === "toolResult" && m.toolCallId.startsWith(`${callId}|`));
      if (message?.role !== "toolResult") throw new Error(`no result for ${callId}`);
      const text = message.content.flatMap((part) => part.type === "text" ? [part.text] : []).join("\n");
      return { text, isError: message.isError, details: message.details };
    },
    journal: () => live.session.sessionManager.getEntries()
      .flatMap((entry) => entry.type === "custom" && entry.customType === "corral_code_call"
        ? [entry.data as Record<string, unknown>] : []),
    since: (from) => events.since(from, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events,
    async close() {
      await live.close();
      globalThis.fetch = originalFetch;
      assets.cleanup();
      rmSync(work, { recursive: true, force: true });
    },
  };
}
