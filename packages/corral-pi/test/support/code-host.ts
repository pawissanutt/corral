// A real HostSession with a fake engine, scripted fixture-provider turns and in-process
// fixture tools whose completion the test controls (`fixture_hold`).
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { type BridgeEvent, EventLog } from "../../src/bridge/events.ts";
import { guardedFetch, restrictEnvironment } from "../../src/host/restrict.ts";
import { HostSession } from "../../src/host/session.ts";
import { assistantRole } from "../../src/roles/assistant/index.ts";
import { reviewerRole } from "../../src/roles/reviewer/index.ts";
import type { CapabilitySet, HostIdentity, RoleContext } from "../../src/roles/types.ts";
import { workerRole } from "../../src/roles/worker/index.ts";
import { CorralClient } from "../../src/tools/corral.ts";
import { type FakeStep, type FakeTransport, fakeCodexTransport } from "../fixtures/auth/fake-transport.ts";
import fixtureAuth from "../fixtures/auth/index.ts";
import { type FakeEngine, writeFakeEngine } from "./fake-engine.ts";
import { PACKAGE_ROOT } from "./fixture-install.ts";
import { nextEvent } from "./wait.ts";

export type CodeRole = "worker" | "reviewer" | "assistant";

const IDENTITIES: Record<CodeRole, HostIdentity> = {
  worker: { store: "st-demo", project: "demo", role: "worker", task: "T-683", run: "R1" },
  reviewer: { store: "st-demo", project: "demo", role: "reviewer", task: "T-683", run: "R1" },
  assistant: { store: "st-demo", project: "", role: "assistant", assistant: "A-1" },
};

const roles = { worker: workerRole, reviewer: reviewerRole, assistant: assistantRole };

interface Gate {
  readonly started: Promise<void>;
  release(text: string): void;
}

/** Each `fixture_hold({name})` call waits until the test releases that name. */
class Holds {
  private readonly gates = new Map<string, { started: () => void; release: (text: string) => void; at: Promise<void>; done: Promise<string> }>();

  private gate(name: string) {
    let entry = this.gates.get(name);
    if (!entry) {
      const at = Promise.withResolvers<void>();
      const done = Promise.withResolvers<string>();
      entry = { started: at.resolve, release: done.resolve, at: at.promise, done: done.promise };
      this.gates.set(name, entry);
    }
    return entry;
  }

  wait(name: string): Promise<string> {
    const entry = this.gate(name);
    entry.started();
    return entry.done;
  }

  get(name: string): Gate {
    const entry = this.gate(name);
    return { started: entry.at, release: entry.release };
  }

  releaseAll(): void {
    for (const gate of this.gates.values()) gate.release("fixture closed");
  }
}

export interface CodeFixture {
  readonly host: HostSession;
  readonly events: EventLog;
  readonly engine: FakeEngine;
  readonly cwd: string;
  readonly transport: FakeTransport;
  readonly holds: Holds;
  readonly big: string;
  forbiddenRuns(): number;
  /** Scripts the provider and submits one prompt; resolves after its turn and usage report. */
  turn(steps: readonly FakeStep[], text?: string): Promise<void>;
  /** The model-visible description of the codemode tool as last prepared. */
  description(): string;
  /** The model-visible tool result whose scripted ID is `callId`. */
  result(callId: string): { text: string; isError: boolean; details: unknown };
  journal(): Record<string, unknown>[];
  since(from: number): readonly BridgeEvent[];
  close(): Promise<void>;
}

export interface CodeFixtureOptions {
  readonly engineReplies?: Parameters<typeof writeFakeEngine>[1];
  readonly enabled?: readonly string[];
  readonly catalog?: number;
  readonly extend?: (pi: ExtensionAPI) => void;
}

export async function codeHost(role: CodeRole, options: CodeFixtureOptions = {}): Promise<CodeFixture> {
  const work = mkdtempSync(join(tmpdir(), `corral-code-${role}-`));
  const originalFetch = globalThis.fetch;
  const cwd = join(work, "cwd");
  mkdirSync(cwd);
  for (const name of ["one.txt", "two.txt"]) {
    writeFileSync(join(cwd, name), `${"discard this line\n".repeat(50)}keep: ${name}\n`);
  }
  restrictEnvironment(process.env);
  const identity = IDENTITIES[role];
  const transport = fakeCodexTransport();
  const network = guardedFetch(transport.send);
  globalThis.fetch = network.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const events = new EventLog();
  const engine = writeFakeEngine(work, options.engineReplies);
  const holds = new Holds();
  const big = "x".repeat(400 * 1024);
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
    {
      name: "fixture_fail", label: "Fail", description: "Returns a nested tool error.", parameters: Type.Object({}),
      async execute() {
        return { content: [{ type: "text", text: "fixture failure" }], details: undefined, isError: true };
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
  const base = roles[role](PACKAGE_ROOT);
  const wrapped = { auth: fixtureAuth, role: { ...base,
    localToolNames: [...(base.localToolNames ?? []), ...local.map((tool) => tool.name)],
    localTools: (context: RoleContext) => [...(base.localTools?.(context) ?? []), ...local],
    extend(pi: Parameters<NonNullable<typeof base.extend>>[0], context: RoleContext) {
      base.extend?.(pi, context);
      options.extend?.(pi);
      pi.registerTool(forbidden);
    },
  } };
  const capabilities: CapabilitySet = { catalog: options.catalog ?? 4, digest: "a".repeat(64), preset: "standard",
    enabled: [...(options.enabled ?? ["codemode"])] };
  const client = new CorralClient({ corral: engine.path, root: work, cwd, identity });
  const live = await HostSession.create({
    // Pi only registers tools in its SDK allowlist. Admit this one there, but not to the host loadout.
    sdk: { ...sdk, createAgentSessionFromServices(opts) {
      return sdk.createAgentSessionFromServices({ ...opts, tools: [...opts.tools ?? [], forbidden.name] });
    } }, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", capabilities, ui: "plain",
    guidance: { projectText: false, settings: {} }, modules: wrapped, events, network, client,
    sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
  // A headless binding, like the CLI's, makes Pi emit session_start on reload.
  await live.session.bindExtensions({ onError: error => { throw error.error; } });
  await live.startLocal();
  return {
    host: live, events, engine, transport, cwd, holds, big,
    forbiddenRuns: () => forbiddenRuns,
    async turn(steps, text = "go") {
      transport.script(steps);
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      const submitted = await live.submitLocal({ text, busyPolicy: "prompt", origin: "owner" });
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
      try {
        const closed = live.close();
        holds.releaseAll();
        await closed;
      } finally {
        globalThis.fetch = originalFetch;
        rmSync(work, { recursive: true, force: true });
      }
    },
  };
}
