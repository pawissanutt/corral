// Code Mode and direct tools share a real host loadout. A cell can compose structured engine
// results with file reads, but cannot reach another role's tools or escape a reload.
import { expect, test } from "bun:test";
import { codeHost } from "./support/code-host.ts";

const artifact = { ref: "T-683:1", metadata: { kind: "evidence" } };

for (const role of ["worker", "reviewer", "assistant"] as const) {
  test(`${role}: code composes calls under the direct loadout and resets on reload`, async () => {
    const fx = await codeHost(role, { engineReplies: { "artifact add": { result: artifact } } });
    const foreign = role === "worker" ? "corral_verdict" : "corral_done";
    const canAttach = role !== "assistant";
    const code = `
      const reads = await Promise.all(["one.txt", "two.txt"].map(path => tools.read({ path })));
      let kept = reads.length;
      const attached = ${canAttach
        ? 'await tools.corral_artifact({path: "one.txt", kind: "evidence"})'
        : 'null'};
      const batch = await Promise.allSettled([
        tools.fixture_fail({}),
        tools.fixture_forbidden({}),
        tools.${foreign}({}),
      ]);
      print(JSON.stringify(batch[1].reason));
      return { kept, kind: attached?.metadata.kind ?? null };
    `;
    try {
      await fx.turn([
        { toolCalls: [{ callId: "c1", name: "codemode", arguments: { code } }] },
        { toolCalls: [{ callId: "c2", name: "codemode", arguments: { code: "return kept;" } }] },
        ...(canAttach ? [{ toolCalls: [{ callId: "direct", name: "corral_artifact",
          arguments: { path: "two.txt", kind: "evidence" } }] }] : []),
        { text: "Finished" },
      ]);

      const callable = (fx.description().split("Callable as tools.<name>: ")[1] ?? "").replace(/\.$/, "").split(", ");
      expect(callable).toContain("read");
      expect(callable).toContain("fixture_fail");
      expect(callable.includes("corral_artifact")).toBe(canAttach);
      for (const name of ["fixture_forbidden", "codemode", foreign]) expect(callable).not.toContain(name);
      const first = fx.result("c1");
      expect(first.isError).toBeFalse();
      expect(first.text).toContain(`Return value: {\n  "kept": 2,\n  "kind": ${canAttach ? '"evidence"' : "null"}\n}`);
      expect(first.text).toContain(`Nested calls: ${canAttach ? 6 : 5} (${canAttach ? 3 : 2} ok, 3 failed).`);
      expect(first.text).toMatch(/- fixture_fail \S+\/cell-1\.\d+: tool_error: fixture failure \[effect: unknown\]/);
      expect(first.text).toMatch(/- fixture_forbidden \S+\/cell-1\.\d+: denied: fixture_forbidden is not in this session's loadout \[effect: not_started\]/);
      expect(first.text).toMatch(new RegExp(`- ${foreign} \\S+/cell-1\\.\\d+: not_run: .* \\[effect: not_started\\]`));
      expect(first.text).toContain(`{"name":"CodeCallError","message":"fixture_forbidden is not in this session's loadout","code":"denied"}`);
      expect(fx.result("c2").text).toContain("Return value: 2");
      expect(fx.forbiddenRuns()).toBe(0);

      const calls = fx.engine.calls().map(argv => argv.slice(3)).filter(argv => argv[0] === "artifact");
      expect(calls).toEqual(canAttach ? ["one.txt", "two.txt"].map(path =>
        ["artifact", "add", "T-683", path, "--kind", "evidence"]) : []);
      const journal = fx.journal().filter(({ parent }) =>
        parent !== null && typeof parent === "object" && "cell" in parent && parent.cell === "cell-1");
      for (const phase of ["started", "settled"]) {
        expect(journal.filter(entry => entry.phase === phase)).toHaveLength(canAttach ? 6 : 5);
      }
      if (canAttach) {
        const attachment = journal.find(entry => entry.phase === "settled" && entry.tool === "corral_artifact");
        expect(attachment).toMatchObject({ ok: true, structured: artifact });
        const parent = attachment?.parent;
        const call = parent && typeof parent === "object" && "tool_call_id" in parent ? parent.tool_call_id : undefined;
        expect(typeof call === "string" && call.startsWith("c1|")).toBeTrue();
      }

      await fx.host.session.reload();
      await fx.turn([{ toolCalls: [{ callId: "c3", name: "codemode", arguments: { code: "return typeof kept;" } }] },
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
      expect(fx.host.loadout).not.toContain("codemode");
      expect(fx.host.session.getAllTools().some(tool => tool.name === "codemode")).toBeFalse();
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
