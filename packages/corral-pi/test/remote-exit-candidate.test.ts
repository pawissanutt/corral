import { expect, test } from "bun:test";
import type { ExitState } from "../src/build/types.ts";
import { boundState, exitBrief, LIVE_STATE_BYTES, type SessionState } from "../src/host/remote/state.ts";

test("a prepared candidate crosses the wire with its exact commits; a host that prepared none sends none", () => {
  const exit: ExitState = {
    phase: "pending",
    candidate: { state: "ready", base: "main", baseTip: "1".repeat(40), head: "2".repeat(40), source: "3".repeat(40), gateAttempt: "G4" },
  };
  expect(exitBrief(exit)).toMatchObject({
    phase: "pending", base_branch: "main",
    candidate: { state: "ready", base: "main", base_tip: "1".repeat(40), head: "2".repeat(40), source: "3".repeat(40), gate_attempt: "G4" },
  });
  expect("candidate" in exitBrief({ phase: "pending" })).toBeFalse();
});

test("conflicted paths are bounded, and go before the exit itself when the live state is too large", () => {
  const conflicts = Array.from({ length: 40 }, (_, index) => `src/${"x".repeat(400)}/${index}.ts`);
  const brief = exitBrief({ phase: "pending", candidate: { state: "conflicts", base: "main", conflicts, detail: "Rebasing onto main conflicts." } });
  expect(brief.candidate?.conflicts).toHaveLength(20);
  for (const path of brief.candidate?.conflicts ?? []) expect(Buffer.byteLength(path)).toBeLessThanOrEqual(240);

  const state = {
    mode: "interactive", turn: 0, open: false, paused: false, effort_levels: [],
    usage: { tokens: 0, cost_partial: false, context_tokens: 0, context_window: 0 }, queue: [], queue_total: 0,
    cards: [], exit: brief, last_error: "e".repeat(10_000),
    children: { live: 0, waiting: 0, total: 0 }, jobs: { live: 0, waiting: 0, total: 0 }, notices: [], can: [],
  } as SessionState;
  const bounded = boundState(state);
  expect(bounded.bytes).toBeLessThanOrEqual(LIVE_STATE_BYTES);
  // The paths went first; the candidate still says it conflicted, and why.
  expect(bounded.state.exit?.candidate?.conflicts).toBeUndefined();
  expect(bounded.state.exit?.candidate).toMatchObject({ state: "conflicts", detail: "Rebasing onto main conflicts." });
});
