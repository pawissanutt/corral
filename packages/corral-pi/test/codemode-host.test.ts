// Code Mode and direct tools share a real host loadout. A cell can compose structured engine
// results with file reads, but cannot reach another role's tools or escape a reload.
import { expect, test } from "bun:test";
import { codeHost } from "./support/code-host.ts";

const applied = { artifact: { task: "T-683", id: 1, kind: "evidence" } };
const reference = "T-683:1";

for (const role of ["worker", "reviewer", "assistant"] as const) {
  test(`${role}: code composes calls under the direct loadout and resets on reload`, async () => {
    const fx = await codeHost(role, { engineReplies: { "artifact add": { result: applied } } });
    const foreign = role === "worker" ? "corral_verdict" : "corral_done";
    const assistant = role === "assistant"; // This fixture has neither a bound task nor a project.
    const code = `
      const reads = await Promise.all(["one.txt", "two.txt"].map(path => tools.read({ path })));
      let kept = reads.length;
      const attached = await tools.corral_artifact({path: "one.txt", kind: "evidence"${assistant ? ', to: "T-683"' : ""}});
      const batch = await Promise.allSettled([
        tools.fixture_fail({}),
        tools.fixture_forbidden({}),
        tools.${foreign}({}),
      ]);
      print(JSON.stringify(batch[1].reason));
      return { kept, kind: attached.artifact.kind };
    `;
    try {
      await fx.turn([
        { toolCalls: [{ callId: "c1", name: "eval_js", arguments: { code } }] },
        { toolCalls: [{ callId: "c2", name: "eval_js", arguments: { code: "return kept;" } }] },
        { toolCalls: [{ callId: "direct", name: "corral_artifact", arguments: assistant
          ? { path: "one.txt", kind: "evidence", to: "T-683", replaces: reference }
          : { path: "two.txt", kind: "evidence" } }] },
        { text: "Finished" },
      ]);

      const advertised = fx.host.session.agent.state.tools.map(tool => tool.name);
      expect(advertised.filter(name => name === "eval_js" || name === "codemode")).toEqual(["eval_js"]);
      const callable = (fx.description().split("Callable as tools.<name>: ")[1] ?? "").replace(/\.$/, "").split(", ");
      expect(callable).toContain("read");
      expect(callable).toContain("fixture_fail");
      expect(callable).toContain("corral_artifact");
      for (const name of ["fixture_forbidden", "eval_js", "codemode", foreign]) expect(callable).not.toContain(name);
      const first = fx.result("c1");
      expect(first.isError).toBeFalse();
      expect(first.text).toContain('Return value: {\n  "kept": 2,\n  "kind": "evidence"\n}');
      expect(first.text).toContain("Nested calls: 6 (3 ok, 3 failed).");
      expect(first.text).toMatch(/- fixture_fail \S+\/cell-1\.\d+: tool_error: fixture failure \[effect: unknown\]/);
      expect(first.text).toMatch(/- fixture_forbidden \S+\/cell-1\.\d+: denied: fixture_forbidden is not in this session's loadout \[effect: not_started\]/);
      expect(first.text).toMatch(new RegExp(`- ${foreign} \\S+/cell-1\\.\\d+: not_run: .* \\[effect: not_started\\]`));
      expect(first.text).toContain(`{"name":"CodeCallError","message":"fixture_forbidden is not in this session's loadout","code":"denied"}`);
      expect(fx.result("c2").text).toContain("Return value: 2");
      expect(fx.forbiddenRuns()).toBe(0);

      const calls = fx.engine.calls().map(argv => argv.slice(3)).filter(argv => argv[0] === "artifact");
      expect(calls).toEqual([
        ["artifact", "add", "T-683", "one.txt", "--kind", "evidence"],
        assistant ? ["artifact", "add", "T-683", "one.txt", "--kind", "evidence", "--replaces", reference]
          : ["artifact", "add", "T-683", "two.txt", "--kind", "evidence"],
      ]);
      const journal = fx.journal().filter(({ parent }) =>
        parent !== null && typeof parent === "object" && "cell" in parent && parent.cell === "cell-1");
      for (const phase of ["started", "settled"]) {
        expect(journal.filter(entry => entry.phase === phase)).toHaveLength(6);
      }
      const attachment = journal.find(entry => entry.phase === "settled" && entry.tool === "corral_artifact");
      expect(attachment).toMatchObject({ ok: true, structured: { ...applied, reference } });
      const parent = attachment?.parent;
      const call = parent && typeof parent === "object" && "tool_call_id" in parent ? parent.tool_call_id : undefined;
      expect(typeof call === "string" && call.startsWith("c1|")).toBeTrue();

      await fx.host.session.reload();
      await fx.turn([{ toolCalls: [{ callId: "c3", name: "eval_js", arguments: { code: "return typeof kept;" } }] },
        { text: "Finished" }]);
      expect(fx.result("c3").text).toContain("Return value: undefined");
    } finally {
      await fx.close();
    }
  }, 120_000);
}

test("a disabled Code Mode leaves direct engine tools available across reload", async () => {
  const fx = await codeHost("worker", { enabled: [] });
  try {
    for (const stage of ["startup", "reload"]) {
      if (stage === "reload") await fx.host.session.reload();
      for (const name of ["eval_js", "codemode"]) {
        expect(fx.host.loadout).not.toContain(name);
        expect(fx.host.session.getAllTools().some(tool => tool.name === name)).toBeFalse();
      }
      await fx.turn([
        { toolCalls: [{ callId: `artifact-${stage}`, name: "corral_artifact",
          arguments: { path: "one.txt", kind: "reference", title: stage } }] },
        { text: "Finished" },
      ]);
      expect(fx.result(`artifact-${stage}`).isError).toBeFalse();
    }
    expect(fx.engine.calls().map(argv => argv.slice(3)).filter(argv => argv[0] === "artifact")).toEqual(
      ["startup", "reload"].map(stage => ["artifact", "add", "T-683", "one.txt", "--kind", "reference", "--title", stage]),
    );
    expect(fx.host.state().codemode).toBeNull();
  } finally {
    await fx.close();
  }
}, 60_000);
