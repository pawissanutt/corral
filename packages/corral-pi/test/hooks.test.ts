// Worker argv hooks (T-572): durable records precede each launch, failures refuse or annotate,
// timeouts kill the hook, and corral operations are never wrapped.
import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { type HookRecord, type HookSettings, HookSettingsSchema, Hooks } from "../src/capabilities/hooks.ts";

const settings = (overrides: Partial<HookSettings>): HookSettings =>
  ({ timeout_ms: 10_000, pre_tool: [], post_tool: [], turn_end: [], ...overrides });

function fixture(hooks: HookSettings) {
  const cwd = mkdtempSync(join(tmpdir(), "corral-hooks-"));
  const records: HookRecord[] = [];
  const launchedBeforeRecord: boolean[] = [];
  const runner = new Hooks(hooks, {
    cwd, generation: 3,
    record: (entry) => {
      if (entry.state === "prepared") launchedBeforeRecord.push(existsSync(join(cwd, "marker")));
      records.push(entry);
    },
  });
  return { cwd, records, launchedBeforeRecord, runner, cleanup: () => rmSync(cwd, { recursive: true, force: true }) };
}

const append = (name: string) => ({ name, argv: ["sh", "-c", `printf '%s %s\\n' "$CORRAL_HOOK_PHASE" "$CORRAL_HOOK_TOOL" >> marker`] });

test("each hook is recorded prepared before launch, then started and completed, with call and turn attribution", async () => {
  // Given a mutating pre hook scoped to read, a post hook and a turn-end hook.
  const fx = fixture(settings({
    pre_tool: [{ ...append("pre"), tools: ["read"] }], post_tool: [append("post")], turn_end: [append("end")],
  }));
  try {
    const call = { tool: "read", toolCallId: "call-1", input: { path: "x" }, turn: 2 };
    // When a read runs through its hooks and the turn ends.
    expect(await fx.runner.before(call)).toBeUndefined();
    expect(await fx.runner.after(call, false)).toEqual([]);
    await fx.runner.turnEnd(2);
    // Then the hooks mutated the worktree, and every launch was preceded by its durable record.
    expect(readFileSync(join(fx.cwd, "marker"), "utf8")).toBe("pre_tool read\npost_tool read\nturn_end \n");
    expect(fx.launchedBeforeRecord).toEqual([false, true, true]);
    expect(fx.records.map((r) => `${r.invocation} ${r.phase} ${r.state}`)).toEqual([
      "hk-3-1 pre_tool prepared", "hk-3-1 pre_tool started", "hk-3-1 pre_tool completed",
      "hk-3-2 post_tool prepared", "hk-3-2 post_tool started", "hk-3-2 post_tool completed",
      "hk-3-3 turn_end prepared", "hk-3-3 turn_end started", "hk-3-3 turn_end completed",
    ]);
    expect(fx.records[0]).toEqual({
      invocation: "hk-3-1", phase: "pre_tool", hook: "pre", tool: "read", tool_call_id: "call-1", turn: 2, state: "prepared",
    });
    expect(fx.records[6]).toEqual({ invocation: "hk-3-3", phase: "turn_end", hook: "end", turn: 2, state: "prepared" });
    expect(fx.records[2]?.exit_code).toBe(0);
    // And scoping keeps the pre hook off other tools and every hook off corral operations.
    expect(fx.runner.wraps("edit")).toBeTrue();
    expect(fx.runner.wraps("corral_done")).toBeFalse();
  } finally {
    fx.cleanup();
  }
});

test("a failing pre hook refuses the call, a failing post hook annotates it, and a hung hook is killed", async () => {
  // Given hooks that fail, cannot start or hang.
  const fx = fixture(settings({
    timeout_ms: 300,
    pre_tool: [{ name: "gate", argv: ["sh", "-c", "echo not today >&2; exit 3"], tools: ["bash"] },
      { name: "missing", argv: ["/nonexistent/corral-hook"], tools: ["edit"] },
      { name: "hang", argv: ["sleep", "30"], tools: ["write"] }],
    post_tool: [{ name: "audit", argv: ["sh", "-c", "exit 1"] }],
  }));
  try {
    const call = (tool: string) => ({ tool, toolCallId: `${tool}-1`, input: {} });
    // Then each failure is reported with its reason and recorded completed.
    expect(await fx.runner.before(call("bash"))).toBe("pre_tool hook gate refused bash: exited 3: not today");
    expect(await fx.runner.before(call("edit"))).toContain("pre_tool hook missing refused edit");
    expect(await fx.runner.before(call("write"))).toBe("pre_tool hook hang refused write: timed out after 300 ms");
    expect(await fx.runner.after(call("read"), true)).toEqual(["[post_tool hook audit: exited 1]"]);
    const completed = fx.records.filter((r) => r.state === "completed");
    expect(completed.map((r) => r.hook)).toEqual(["gate", "missing", "hang", "audit"]);
    expect([completed[2]?.timed_out, completed[2]?.signal]).toEqual([true, "SIGKILL"]);
    expect(completed[1]?.error).toBeDefined();
  } finally {
    fx.cleanup();
  }
});

test("hook settings parse only the resolved Rust shape", () => {
  expect(HookSettingsSchema.safeParse(settings({ pre_tool: [{ name: "a", argv: ["a"], tools: ["read"] }] })).success)
    .toBeTrue();
  for (const invalid of [
    settings({ timeout_ms: 0 }), settings({ pre_tool: [{ name: "a", argv: [] }] }),
    settings({ pre_tool: [{ name: "a", argv: ["a"], tools: ["corral_done"] }] }),
    settings({ turn_end: [{ name: "a", argv: ["a"], tools: ["read"] }] }), { ...settings({}), extra: true },
  ]) {
    expect(HookSettingsSchema.safeParse(invalid).success).toBeFalse();
  }
});
