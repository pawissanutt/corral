import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { executionRefusal } from "../src/roles/common/execution.ts";
import assistant from "../src/roles/assistant/index.ts";
import reviewer from "../src/roles/reviewer/index.ts";
import worker from "../src/roles/worker/index.ts";
import { defineCorralTool } from "../src/tools/define.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { verdictTool } from "../src/roles/reviewer/tools.ts";
import { sessionTodoTool } from "../src/roles/common/todo.ts";
import { validateToolArguments } from "@earendil-works/pi-ai";
import { writeFakeEngine } from "./support/fake-engine.ts";

const roles = [worker, reviewer, assistant];

test("one loadout check decides direct and Code Mode calls; every Corral tool is callable from code", () => {
  for (const role of roles) {
    const identity = { store: "store", project: "project", role: role.role, task: "T-1", run: "R1" };
    const client = new CorralClient({ corral: "", root: ".", cwd: ".", identity });
    const specs = role.tools({ identity, cwd: ".", model: "fixture/model", todo: true });
    const loadout = new Set(specs.map((spec) => spec.name));
    for (const spec of specs) {
      for (const nested of [false, true]) {
        expect(executionRefusal(spec.name, nested, new Set())).toContain("loadout");
        expect(executionRefusal(spec.name, nested, loadout)).toBeUndefined();
      }
      const tool = defineCorralTool(spec, {
        identity, client,
        drained() { throw new Error("This test only checks the tool definition"); },
        todo() { throw new Error("This test only checks the tool definition"); },
        checkpoint() { throw new Error("This test only checks the tool definition"); },
      });
      expect([tool.exposure, tool.description]).toEqual(["direct", spec.description]);
    }
  }
  expect(executionRefusal("codemode", false, new Set(["codemode"]))).toBeUndefined();
  expect(executionRefusal("codemode", true, new Set(["codemode"]))).toContain("codemode");
});

test("a terminal verdict flushes reported usage before an engine that can close the reviewer", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-verdict-"));
  try {
    const engine = writeFakeEngine(root, { "review-verdict": { result: { recorded: true } } });
    const identity = { store: "local", project: "demo", role: "reviewer" as const, task: "T-1", run: "R2" };
    const client = new CorralClient({ corral: engine.path, root, cwd: join(root, "removed"), sessionDir: root, identity });
    const usage = Promise.withResolvers<void>();
    const waiting = Promise.withResolvers<void>();
    const tool = defineCorralTool(verdictTool, {
      identity, client,
      drained() { waiting.resolve(); return usage.promise; },
      todo() { throw new Error("unexpected todo"); },
      checkpoint() { throw new Error("unexpected checkpoint"); },
    });
    const request = `9-0-${"a".repeat(40)}`;
    const verdict = tool.execute("approve", { request, verdict: "approve", notes: "Looks good" }, undefined, undefined, {} as never);
    await Promise.race([waiting.promise, verdict]);
    expect(JSON.parse(readFileSync(join(root, `verdict-${identity.run}-${request}.json`), "utf8")))
      .toEqual({ verdict: "approve", notes: "Looks good" });
    expect(engine.calls()).toEqual([]);
    usage.resolve();
    expect(await verdict).toMatchObject({ terminate: true, details: { recorded: true } });
    expect(engine.calls()).toEqual([["--root", root, "--json", "review-verdict", "T-1", "R2", request]]);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("a verdict without a valid receipt reports the failure without terminating the turn", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-verdict-receipt-"));
  try {
    const identity = { store: "local", project: "demo", role: "reviewer" as const, task: "T-1", run: "R2" };
    const request = `9-0-${"a".repeat(40)}`;
    const reason = "Cannot record the verdict: " + "🔒".repeat(1100);
    const next = [{ cmd: "corral-next show T-1" }];
    for (const failure of [{ result: {} }, { stdout: JSON.stringify({ reason, next }), code: 2 }]) {
      const engine = writeFakeEngine(root, { "review-verdict": failure });
      const tool = defineCorralTool(verdictTool, {
        identity, client: new CorralClient({ corral: engine.path, root, cwd: root, sessionDir: root, identity }),
        async drained() {},
        todo() { throw new Error("unexpected todo"); },
        checkpoint() { throw new Error("unexpected checkpoint"); },
      });
      const result = await tool.execute("approve", { request, verdict: "approve", notes: "Looks good" }, undefined, undefined, {} as never);
      expect(result.isError).toBe(true);
      expect(result.terminate).toBeUndefined();
      const bytes = readFileSync(join(root, `verdict-${identity.run}-${request}.error`));
      expect(bytes.length).toBeLessThanOrEqual(4096);
      const diagnostic = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      if (failure.code) {
        expect(diagnostic).toContain("Cannot record the verdict");
        expect(result.details).toEqual({ reason, next });
      }
      else expect(diagnostic).toContain("expected recorded: true");
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("corral_todo reaches every provider as one root object that names its fields", async () => {
  // Anthropic replaced the root union with an empty object, so models guessed every argument.
  const identity = { store: "store", project: "project", role: "worker" as const, task: "T-1", run: "R1" };
  const calls: Record<string, unknown>[] = [];
  const tool = defineCorralTool(sessionTodoTool, {
    identity, client: new CorralClient({ corral: "", root: ".", cwd: ".", identity }),
    drained() { throw new Error("unexpected drain"); },
    todo(payload) { calls.push(payload); return { plan: "P1" }; },
    checkpoint() { throw new Error("unexpected checkpoint"); },
  });
  const schema = tool.parameters as unknown as { type: string; properties: Record<string, unknown>; required: string[] };
  expect([schema.type, schema.required]).toEqual(["object", ["op"]]);
  expect(["op", "phases", "step", "on"].filter((key) => !(key in schema.properties))).toEqual([]);
  // A stringified array argument is parsed before Pi validates the call.
  const phases = [{ id: "a", label: "A", steps: [{ id: "s", label: "S" }] }];
  const args = validateToolArguments(tool as never, { type: "toolCall", id: "1", name: tool.name,
    arguments: tool.prepareArguments!({ op: "init", phases: JSON.stringify(phases) }) as never });
  await tool.execute("1", args, undefined, undefined, undefined as never);
  expect(calls).toEqual([{ op: "init", phases }]);
  // The exact union still decides, and names the fields of the op the call chose.
  const refused = await tool.execute("2", { op: "init" }, undefined, undefined, undefined as never);
  expect(refused.isError).toBe(true);
  expect(JSON.stringify(refused.details)).toContain("init takes {op, phases}");
});
