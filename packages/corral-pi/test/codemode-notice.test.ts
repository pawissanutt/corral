// A detached cell's one notice keeps what acceptance makes mandatory (T-683 review F1): its status,
// the bounded nested-failure summary with tool, child identity, code and effect, and its return
// value, however much the cell printed. Only printed output yields to the notice budget, and the
// notice says how much it left out. Ordinary and reload-deferred delivery share this path; an inline
// result is not shortened.
import { expect, test } from "bun:test";

import { NOTICE_MAX_BYTES } from "../src/extensions/codemode/report.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { codeHost } from "./support/code-host.ts";
import { nextEvent } from "./support/wait.ts";

const PRINTED = 40_000;
const VALUE = `${"V".repeat(15_000)}END`;
const failures = `
  const settled = await Promise.allSettled([
    ...Array.from({ length: 9 }, () => tools.fixture_forbidden({})),
    tools.corral_withdraw({ key: "T-683/R1/Q9", reason: "obsolete" }),
  ]);
  print("x".repeat(${PRINTED}));
`;

function notices(fx: Awaited<ReturnType<typeof codeHost>>): string[] {
  return fx.host.session.messages.filter((message) => message.role === "user")
    .map((message) => typeof message.content === "string" ? message.content
      : message.content.map((part) => part.type === "text" ? part.text : "").join(""))
    .filter((text) => text.startsWith("[corral code cell"));
}

function expectMandatory(notice: string, status: string): void {
  expect(Buffer.byteLength(notice) <= NOTICE_MAX_BYTES).toBe(true);
  expect(notice).toContain(`[corral code cell cell-1 settled: ${status}]`);
  expect(notice).toContain("Nested failures (10):");
  expect(notice).toMatch(/- fixture_forbidden \S+\/cell-1\.\d+: denied: fixture_forbidden is not in this session's loadout \[effect: not_started\]/);
  // Failures past the eighth still name their tools.
  expect(notice).toContain("- ... and 2 more: fixture_forbidden (1), corral_withdraw (1)");
  expect(notice).toMatch(new RegExp(`\\[Printed output clipped in this message: \\d+ of ${PRINTED} bytes shown\\.`));
}

test("a detached notice keeps every mandatory section and the return value past its output budget", async () => {
  const fx = await codeHost("worker");
  try {
    fx.corral.rules({ withdraw: { reject: { code: "stale_revision", reason: "the item moved" } } });
    // When a detached cell catches ten failed calls, prints far past the budget and returns a large value:
    const code = `await tools.fixture_hold({ name: "gate" });${failures}return ${JSON.stringify(VALUE)};`;
    await fx.turn([{ toolCalls: [{ callId: "c1", name: "codemode", arguments: { code, wait_ms: 0 } }] }, { text: "Waiting" }]);
    fixtureTransport().script([{ text: "Noted" }]);
    const ended = nextEvent(fx.events, (event) => event.kind === "turn_ended");
    fx.holds.get("gate").release("go");
    await ended;
    await fx.host.drained();
    // Then its one notice is within budget, keeps status, summary and the whole value, and says what it clipped:
    const [notice, ...more] = notices(fx);
    expect(more).toEqual([]);
    expectMandatory(notice ?? "", "ok");
    expect(notice).toContain("Nested calls: 11 (1 ok, 10 failed).");
    expect(notice).toContain(`Return value: ${VALUE}`);
  } finally {
    await fx.close();
  }
}, 60_000);

test("a reload-deferred notice keeps the mandatory sections too, and an inline result is not shortened", async () => {
  const fx = await codeHost("worker");
  try {
    fx.corral.rules({ withdraw: { reject: { code: "stale_revision", reason: "the item moved" } } });
    // When a cell that already caught its failures and printed is closed by a reload while it waits:
    const code = `${failures}await tools.fixture_hold({ name: "late" });`;
    await fx.turn([{ toolCalls: [{ callId: "c1", name: "codemode", arguments: { code, wait_ms: 0 } }] }, { text: "Waiting" }]);
    await fx.holds.get("late").started;
    fixtureTransport().script([{ text: "Noted" }]);
    const notified = nextEvent(fx.events, (event) => event.kind === "code_cell_notified");
    await fx.host.session.reload();
    await notified;
    await fx.host.drained();
    // Then the deferred notice keeps them as well:
    const [notice] = notices(fx);
    expectMandatory(notice ?? "", "closed");
    expect(notice).toContain("every variable defined before is gone");
    // And an inline report keeps the cell's whole printed output (bounded by the kernel, not the notice).
    await fx.turn([{ toolCalls: [{ callId: "c2", name: "codemode", arguments: { code: `print("y".repeat(${PRINTED}));` } }] },
      { text: "Finished" }]);
    const inline = fx.result("c2").text;
    expect(inline).toContain("y".repeat(PRINTED));
    expect(inline).not.toContain("clipped in this message");
    fx.holds.get("late").release("after");
  } finally {
    await fx.close();
  }
}, 60_000);
