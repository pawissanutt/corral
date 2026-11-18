import { expect, test } from "bun:test";
import type { BuildGateResult, RecordReview } from "../../build/record-types.ts";
import type { BodyLine, CallInput, RowModel } from "../tool-row.ts";
import { buildPresenters } from "./build-presenters.ts";

function present(name: string, details: unknown, isError = false): RowModel {
  const call: CallInput = { name, args: {}, argsComplete: true, executionStarted: true, partial: false, isError, expanded: false, cwd: "/w",
    toolCallId: "call-1", result: { content: [{ type: "text", text: "worktree_dirty: 2 files" }], details } };
  return buildPresenters.presenter(name, undefined as never)!(call);
}
const gate = (result: BuildGateResult["result"], extra: Partial<BuildGateResult> = {}): BuildGateResult => ({
  attempt: "G2", head: "abc", result, commands: ["just check"], started: "2026-01-01T00:00:00Z", ended: "2026-01-01T00:01:00Z", ...extra,
});

test("a gate result decides the row: a failure names its command, a skipped gate never reads as the pass, a refusal fails", () => {
  const failed = present("build_verify", { gate: gate("fail", { failed_command: "FAILED_COMMAND" }) });
  expect(failed.state).toBe("failed");
  expect(failed.body?.some(line => line.text.includes("FAILED_COMMAND"))).toBeTrue();
  const passed = present("build_verify", { gate: gate("pass") }), skipped = present("build_verify", { gate: gate("skipped") });
  expect(passed.state).toBe("ok");
  expect(passed.right).toBe("G2");
  expect(skipped.outcome).not.toBe(passed.outcome);
  expect(present("build_verify", {}, true).state).toBe("failed");
  expect(present("build_verify", {}).state).toBe("unknown");
});

test("a review row carries its round and, expanded, every finding", () => {
  const review: RecordReview = { round: 2, reviewer: "review-2", model: "p/m", head: "abc", verdict: "changes", at: "t",
    findings: [{ id: "F1", severity: "blocker", path: "src/a.ts", line: 7, text: "FINDING_TEXT" }] };
  const row = present("build_review", { round: 2, review });
  expect(row.state).toBe("ok");
  expect(row.right).toContain("2");
  const expanded = row.expanded?.(80) as readonly BodyLine[];
  expect(expanded.some(line => line.text.includes("src/a.ts:7") && line.text.includes("FINDING_TEXT"))).toBeTrue();
  expect(buildPresenters.presenter("agent_message", undefined as never)).toBeUndefined();
});
