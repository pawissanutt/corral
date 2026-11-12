import { expect, test } from "bun:test";
import { AuxiliaryRuntime, ShadowDispatcher, SHADOW_PROMPT, type Invocation } from "./runtime.ts";
import { auxiliarySettings } from "./settings.ts";

const shadow = { model: "cheap", timeout_secs: 60, max_per_hour: 30 };
const card = { kind: "dispatch", task: "T-1", why: "ready", evidence: ["gate passed"],
  options: [{ id: "dispatch", label: "Dispatch", command: "secret-command", requires_owner: false },
    { id: "wait", label: "Wait", command: "other-command", requires_owner: false }],
  recommended: "dispatch", recommendation_marker: "never disclose" };
const brief = { project: "demo", counts: {}, questions: [], next: card, shadow_digest: "digest-1" };
function fixture(output = '{"choice":"dispatch","reason":"ready"}', cap = 30, enabled = true) {
  const invocations: Invocation[] = [];
  const lifecycle: Record<string, unknown>[] = [];
  const calls: { op: string; payload: Record<string, unknown> }[] = [];
  const diagnostics: unknown[] = [];
  let current: unknown = brief;
  let now = 0;
  let response = output;
  const settings = auxiliarySettings("pm", enabled ? ["shadow"] : [], { shadow });
  const runtime = new AuxiliaryRuntime({ settings, generation: 7, provider: "fixture", model: "parent",
    sessionId: () => "parent",
    async record(payload) { lifecycle.push(payload); return { session_id: payload.request, session_file: "/unused" }; },
    async execute(invocation) { invocations.push(invocation); return { text: response, outcome: "completed" }; },
  });
  const dispatcher = settings.shadow ? new ShadowDispatcher({ runtime, model: "fixture/cheap", maxPerHour: cap,
    now: () => now, diagnostic: error => diagnostics.push(error),
    async call(op, payload) { calls.push({ op, payload }); return op === "read_brief" ? current : {}; },
  }) : undefined;
  return { runtime, dispatcher, invocations, lifecycle, calls, diagnostics,
    brief(value: unknown) { current = value; }, clock(value: number) { now = value; }, output(value: string) { response = value; },
    async start(origin: "inbox" | "continuation" | "other" = "inbox") { dispatcher?.start(origin); await dispatcher?.settled(); },
  };
}

test("shadow disabled by default and non-inbox turns start nothing", async () => {
  expect(auxiliarySettings("pm", []).shadow).toBeUndefined();
  expect(auxiliarySettings("worker", ["shadow"], { shadow }).shadow).toBeUndefined();
  const disabled = fixture(undefined, 30, false);
  await disabled.start();
  expect(disabled.calls).toHaveLength(0);
  expect("shadow" in disabled.runtime.state()).toBe(false);
  expect(disabled.invocations).toHaveLength(0);
  const enabled = fixture();
  expect(enabled.runtime.state().shadow).toEqual({ running: 0, queued: 0 });
  await enabled.start("continuation");
  await enabled.start("other");
  expect(enabled.calls).toHaveLength(0);
  expect(enabled.invocations).toHaveLength(0);
});

test("shadow deduplicates digest per generation and publishes only the fixed payload", async () => {
  const fx = fixture();
  fx.dispatcher?.start("inbox");
  fx.dispatcher?.start("inbox");
  await fx.dispatcher?.settled();
  expect(fx.invocations).toHaveLength(1);
  const invocation = fx.invocations[0]!;
  expect(invocation).toMatchObject({ kind: "shadow", model: "cheap", provider: "fixture", maxTurns: 1 });
  expect(JSON.parse(invocation.prompt.slice(SHADOW_PROMPT.length))).toEqual({
    kind: card.kind, task: card.task, why: card.why, evidence: card.evidence,
    options: card.options.map(({ id, label }) => ({ id, label })),
  });
  expect(invocation.prompt).not.toContain("recommended");
  expect(invocation.prompt).not.toContain("never disclose");
  expect(invocation.prompt).not.toContain("secret-command");
  expect(fx.calls.filter(call => call.op === "pm_shadow")).toEqual([{ op: "pm_shadow", payload: {
    digest: "digest-1", choice: "dispatch", requested_escalation: false, model: "fixture/cheap", reason: "ready",
  } }]);
  fx.brief({ ...brief, shadow_digest: "digest-2" });
  await fx.start();
  expect(fx.invocations).toHaveLength(2);
  const nextGeneration = fixture();
  await nextGeneration.start();
  expect(nextGeneration.invocations).toHaveLength(1);
  fx.brief({ ...brief, shadow_digest: undefined });
  await fx.start();
  expect(fx.invocations).toHaveLength(2);
});

test("malformed or out-of-menu shadow output records uncertain lifecycle but no decision", async () => {
  const fx = fixture();
  for (const [index, text] of ["not json", '```json\n{"choice":"dispatch","reason":"ok"}\n```',
    '{"choice":"invented","reason":"ok"}', '{"choice":"dispatch","reason":"ok","extra":true}',
    '{"choice":"dispatch"}'].entries()) {
    fx.output(text);
    fx.brief({ ...brief, shadow_digest: `invalid-${index}` });
    await fx.start();
  }
  expect(fx.calls.filter(call => call.op === "pm_shadow")).toHaveLength(0);
  expect(fx.lifecycle.filter(event => event.action === "finish").map(event => event.outcome)).toEqual(Array(5).fill("uncertain"));
  expect(fx.diagnostics).toHaveLength(5);
});

test("shadow hourly cap counts attempts and opens after one hour", async () => {
  const fx = fixture("invalid", 1);
  await fx.start();
  fx.brief({ ...brief, shadow_digest: "second" });
  await fx.start();
  expect(fx.invocations).toHaveLength(1);
  fx.clock(3600000);
  fx.output('{"choice":"escalate","reason":"owner-only"}');
  await fx.start();
  expect(fx.invocations).toHaveLength(2);
  expect(fx.calls.at(-1)?.payload).toEqual({ digest: "second", choice: "escalate",
    requested_escalation: true, model: "fixture/cheap", reason: "owner-only" });
});

test("shadow reason is truncated by UTF-8 bytes and dedupe storage stops at 256", async () => {
  const fx = fixture(JSON.stringify({ choice: "dispatch", reason: "€".repeat(200) }), 600);
  for (let index = 0; index < 257; index++) {
    fx.brief({ ...brief, shadow_digest: `digest-${index}` });
    await fx.start();
  }
  expect(fx.invocations).toHaveLength(256);
  expect(fx.calls.filter(call => call.op === "pm_shadow")[0]?.payload.reason).toBe("€".repeat(166));
  fx.brief(brief);
  await fx.start();
  expect(fx.invocations).toHaveLength(256);
});

test("host replacement aborts in-flight shadow without delaying its caller or publishing", async () => {
  const { promise: began, resolve: started } = Promise.withResolvers<void>();
  let aborted = false;
  const events: Record<string, unknown>[] = [];
  const runtime = new AuxiliaryRuntime({ settings: { shadow }, generation: 1, provider: "fixture", model: "parent",
    sessionId: () => "parent",
    async record(payload) { events.push(payload); return { session_id: payload.request, session_file: "/unused" }; },
    async execute(_invocation, signal) {
      started();
      const { promise, reject } = Promise.withResolvers<void>();
      signal.addEventListener("abort", () => { aborted = true; reject(signal.reason); }, { once: true });
      await promise;
      return { text: "unreachable", outcome: "completed" };
    },
  });
  const decisions: unknown[] = [];
  const diagnostics: unknown[] = [];
  const dispatcher = new ShadowDispatcher({ runtime, model: "fixture/cheap", maxPerHour: 30,
    diagnostic: error => diagnostics.push(error), async call(op, payload) {
      if (op === "pm_shadow") decisions.push(payload);
      return brief;
    },
  });
  expect(dispatcher.start("inbox")).toBeUndefined();
  await began;
  dispatcher.cancel();
  await dispatcher.settled();
  expect(aborted).toBeTrue();
  expect(decisions).toHaveLength(0);
  expect(diagnostics).toHaveLength(1);
  expect(events.at(-1)?.outcome).toBe("uncertain");
  await runtime.close();
});

test("a refused shadow recording is diagnosed once without retrying or affecting the caller", async () => {
  let writes = 0;
  const diagnostics: unknown[] = [];
  const runtime = new AuxiliaryRuntime({ settings: { shadow }, generation: 1, provider: "fixture", model: "parent",
    sessionId: () => "parent",
    async record(payload) { return { session_id: payload.request, session_file: "/unused" }; },
    async execute() { return { text: '{"choice":"wait","reason":"unsure"}', outcome: "completed" }; },
  });
  const dispatcher = new ShadowDispatcher({ runtime, model: "fixture/cheap", maxPerHour: 30,
    diagnostic: error => diagnostics.push(error), async call(op) {
      if (op === "pm_shadow") { writes++; throw new Error("op refused"); }
      return brief;
    },
  });
  dispatcher.start("inbox");
  await dispatcher.settled();
  dispatcher.start("inbox");
  await dispatcher.settled();
  expect(writes).toBe(1);
  expect(diagnostics).toHaveLength(1);
  await runtime.close();
});

test("shadow reads one brief per inbox turn and ignores missing or ineligible next cards", async () => {
  const fx = fixture();
  for (const result of [
    { ...brief, next: null },
    { ...brief, next: undefined },
    { ...brief, next: { ...card, options: undefined } },
    { ...brief, next: { ...card, options: [] } },
    { ...brief, shadow_digest: undefined },
    // A bare read_next-style card is not the shadow transport.
    { ...card, shadow_digest: brief.shadow_digest },
  ]) {
    fx.brief(result);
    await fx.start();
  }
  expect(fx.calls).toEqual(Array.from({ length: 6 }, () => ({ op: "read_brief", payload: {} })));
  expect(fx.invocations).toHaveLength(0);
  expect(fx.diagnostics).toHaveLength(0);
});
