// Code Mode on the Corral kernel through the real HostSession, per role (T-683): the callable list
// and nested execution follow one loadout check; parallel reads and a dependent read -> mutation ->
// read pipeline run through the real host bridge with parent-bound receipts and a session journal;
// refusals and rejected receipts in a caught batch still reach the model-visible summary; direct
// calls keep working; persistence survives cells but not a reload. Fixture provider, fake corral.
import { expect, test } from "bun:test";

import { toolRequestId } from "../src/tools/corral.ts";
import { type CodeRole, codeHost } from "./support/code-host.ts";

interface Case {
  readonly reads: string;
  readonly readCount: number;
  readonly mutation: { readonly tool: string; readonly op: string; readonly args: Record<string, unknown> };
  readonly rejected: { readonly tool: string; readonly op: string; readonly args: Record<string, unknown> };
  readonly lookup: Record<string, unknown>;
  /** A Corral tool another role has: it is not in this loadout, so code cannot reach it either. */
  readonly foreign: string;
}

const FILE_READS = `Promise.all(["one.txt", "two.txt"].map(path => tools.read({ path })))`;
const CASES: Record<CodeRole, Case> = {
  worker: {
    reads: FILE_READS, readCount: 2, lookup: {}, foreign: "corral_comment",
    mutation: { tool: "corral_ask", op: "ask", args: { kind: "clarification", text: "Which input?" } },
    rejected: { tool: "corral_withdraw", op: "withdraw", args: { key: "T-683/R1/Q9", reason: "obsolete" } },
  },
  pm: {
    reads: `Promise.all([tools.corral_brief({}), tools.corral_next({}), tools.corral_inbox_read({})])`, readCount: 3,
    lookup: { task: "T-9" }, foreign: "corral_done",
    mutation: { tool: "corral_comment", op: "comment", args: { task: "T-9", text: "noted from code" } },
    rejected: { tool: "corral_capture", op: "capture", args: { project: "demo", title: "Idea", body: "Body" } },
  },
  assistant: {
    reads: FILE_READS, readCount: 2, lookup: {}, foreign: "corral_done",
    mutation: { tool: "corral_owner_action", op: "owner_action", args: { kind: "approve", target: "T-5", input: "A-1/g1/i1" } },
    rejected: { tool: "corral_owner_action", op: "owner_action", args: { kind: "retry", target: "T-6", input: "A-1/g1/i1" } },
  },
};

for (const role of ["worker", "pm", "assistant"] as const) {
  test(`${role}: code composes reads and receipted mutations under direct authority, across a reload`, async () => {
    const spec = CASES[role];
    const fx = await codeHost(role);
    try {
      fx.corral.rules({ [spec.rejected.op]: [{ match: { target: "T-6" }, reject: { code: "stale_revision", reason: "the item moved" } },
        ...(spec.rejected.op === spec.mutation.op ? [] : [{ reject: { code: "stale_revision", reason: "the item moved" } }])] });
      const lookupArgs = JSON.stringify(spec.lookup);
      const code = `
        const reads = await ${spec.reads};
        let kept = { reads: reads.length };
        const made = await tools.${spec.mutation.tool}(${JSON.stringify(spec.mutation.args)});
        const back = await tools.corral_receipt_lookup({ request_id: made.request_id, ...${lookupArgs} });
        const batch = await Promise.allSettled([
          tools.${spec.rejected.tool}(${JSON.stringify(spec.rejected.args)}),
          tools.fixture_forbidden({}),
          tools.${spec.foreign}({}),
        ]);
        print("batch " + JSON.stringify(batch.map(r => r.status === "rejected" ? r.reason : r.value.state)));
        return { made: made.state, back: back.lookup.found, call: back.identity.parent.call };
      `;
      // When the model runs one cell, a second cell, then the mutation tool directly:
      await fx.turn([
        { toolCalls: [{ callId: "c1", name: "codemode", arguments: { code } }] },
        { toolCalls: [{ callId: "c2", name: "codemode", arguments: { code: "return kept.reads;" } }] },
        { toolCalls: [{ callId: "direct", name: spec.mutation.tool, arguments: spec.mutation.args }] },
        { text: "Finished" },
      ]);

      // Then discovery lists exactly what the loadout permits:
      const callable = (fx.description().split("Callable as tools.<name>: ")[1] ?? "").replace(/\.$/, "").split(", ");
      for (const name of [spec.mutation.tool, spec.rejected.tool, "corral_receipt_lookup", "fixture_hold"]) {
        expect(callable).toContain(name);
      }
      for (const name of ["fixture_forbidden", "codemode", spec.foreign]) expect(callable).not.toContain(name);

      // And the cell succeeded while its nested failures stay separately visible and bounded:
      const first = fx.result("c1");
      expect(first.isError).toBe(false);
      expect(first.text).toContain(`Return value: {\n  "made": "committed",\n  "back": true,\n  "call": "cell-1.`);
      expect(first.text).toContain(`Nested calls: ${spec.readCount + 5} (${spec.readCount + 2} ok, 3 failed; 1 committed receipts stand).`);
      expect(first.text).toMatch(new RegExp(`- ${spec.rejected.tool} \\S+/cell-1\\.\\d+ rq-[0-9a-f]{32}: stale_revision: the item moved \\[effect: rejected\\]`));
      expect(first.text).toMatch(/- fixture_forbidden \S+\/cell-1\.\d+: denied: fixture_forbidden is not in this session's loadout \[effect: not_started\]/);
      expect(first.text).toMatch(new RegExp(`- ${spec.foreign} \\S+/cell-1\\.\\d+: not_run: .* \\[effect: not_started\\]`));
      // A caught refusal still carries its name, message and code through JSON.stringify:
      expect(first.text).toContain(`{"name":"CodeCallError","message":"fixture_forbidden is not in this session's loadout","code":"denied"}`);
      expect(first.text).not.toContain("{}");
      expect(fx.result("c2").text).toContain(`Return value: ${spec.readCount}`);

      // And the nested mutation is one receipt bound to its parent cell and transcript call:
      const sessionId = fx.host.sessionId();
      const calls = fx.corral.calls().filter((envelope) => envelope.op === spec.mutation.op
        && envelope.payload.target !== "T-6");
      expect(calls).toHaveLength(2);
      const [nested, direct] = calls;
      const parent = nested?.identity.parent;
      expect(parent?.tool_call_id.startsWith("c1|")).toBe(true);
      expect(parent?.cell).toBe("cell-1");
      expect(nested?.request_id).toBe(toolRequestId(sessionId, `${parent?.tool_call_id}/${parent?.call}`));
      expect(direct?.identity.parent).toBeUndefined();
      expect(direct?.request_id).toMatch(/^rq-[0-9a-f]{32}$/);
      expect(direct?.request_id).not.toBe(nested?.request_id);
      expect(fx.corral.effects().filter((effect) => effect.op === spec.mutation.op && effect.payload.target !== "T-6")
        .map((effect) => effect.parent?.call ?? null)).toEqual([parent?.call ?? "", null]);
      expect(fx.forbiddenRuns()).toBe(0);
      expect(fx.corral.calls().some((envelope) => envelope.op === "done")).toBe(false);

      // And every child was journaled before it ran and when it settled, under its parent:
      const journal = fx.journal().filter((entry) => (entry.parent as { cell?: string }).cell === "cell-1");
      const started = journal.filter((entry) => entry.phase === "started");
      expect(started).toHaveLength(spec.readCount + 5);
      expect(journal.filter((entry) => entry.phase === "settled")).toHaveLength(spec.readCount + 5);
      expect(journal.find((entry) => entry.phase === "settled" && entry.tool === spec.mutation.tool))
        .toMatchObject({ ok: true, effect: "committed", request_id: nested?.request_id, generation: 1 });

      // When the session reloads, the replacement kernel starts empty:
      await fx.host.session.reload();
      await fx.turn([{ toolCalls: [{ callId: "c3", name: "codemode", arguments: { code: "return typeof kept;" } }] },
        { text: "Finished" }]);
      expect(fx.result("c3").text).toContain("Return value: undefined");
      expect(fx.description()).toContain(spec.mutation.tool);
    } finally {
      await fx.close();
    }
  }, 120_000);
}

test("a disabled capability leaves no code tool while direct Corral tools still work, across reload", async () => {
  const fx = await codeHost("worker", { enabled: [] });
  try {
    for (const stage of ["startup", "reload"]) {
      if (stage === "reload") await fx.host.session.reload();
      expect(fx.host.loadout).not.toContain("codemode");
      expect(fx.host.session.getAllTools().some((tool) => tool.name === "codemode")).toBe(false);
      await fx.turn([{ toolCalls: [{ callId: `ask-${stage}`, name: "corral_ask",
        arguments: { kind: "clarification", text: stage } }] }, { text: "Finished" }]);
    }
    expect(fx.corral.effects().map((effect) => [effect.op, effect.payload.text, effect.parent])).toEqual([
      ["ask", "startup", null], ["ask", "reload", null],
    ]);
    expect(fx.host.state().codemode).toBeNull();
  } finally {
    await fx.close();
  }
}, 60_000);
