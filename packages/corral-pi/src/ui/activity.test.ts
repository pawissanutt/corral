import { afterEach, expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { Activity, ActivityClock, PassCache, STALL_MS, STILL_TICK_MS, TICK_MS, type Scheduler } from "./activity.ts";
import { composerTitle, composerTop } from "./composer-chrome.ts";
import { setMotion } from "./frame.ts";
import { headerFrame, NOTICE_MS } from "./header.ts";
import { PANEL_BLUE } from "./panels-view.ts";

const SPIN = /[⠋⠙⠹⠸⠼⠴⠦⠧]/;
afterEach(() => setMotion("animate"));

/** A scheduler whose time moves only when the test advances it; due ticks run in order. */
function fakeTime(start = 1_700_000_000_000) {
  let now = start;
  const timers = new Set<{ readonly ms: number; readonly tick: () => void; next: number }>();
  const scheduler: Scheduler = {
    now: () => now,
    every: (ms, tick) => {
      const timer = { ms, tick, next: now + ms };
      timers.add(timer);
      return () => { timers.delete(timer); };
    },
  };
  return {
    scheduler, timers, now: () => now,
    advance(ms: number) {
      const end = now + ms;
      for (;;) {
        const due = [...timers].filter(timer => timer.next <= end).sort((a, b) => a.next - b.next)[0];
        if (!due) break;
        now = due.next; due.next += due.ms; due.tick();
      }
      now = end;
    },
  };
}
function rig() {
  const time = fakeTime(), activity = new Activity(time.now);
  let still = false, ticks = 0;
  const clock = new ActivityClock(time.scheduler, () => activity.busy(), () => { ticks++; }, () => still);
  return { time, activity, clock, ticks: () => ticks, still: (value: boolean) => { still = value; clock.sync(); } };
}
const plain = (text: string) => stripTerminalSequences(text);
const shown = (activity: Activity, columns: number, now: number) => plain(activity.indicator(columns, now, "esc")[0] ?? "");

test("waiting on the model with no events, two frames 300 ms apart differ and the elapsed time advances; idle leaves no interval", () => {
  const { time, activity, clock, ticks } = rig();
  expect(clock.running).toBeFalse();
  activity.agentStart(); activity.request(); clock.sync();
  expect(clock.running).toBeTrue();
  const first = shown(activity, 185, time.now());
  time.advance(300);
  const second = shown(activity, 185, time.now());
  expect(first).not.toBe(second);
  expect(first).toMatch(new RegExp("^" + SPIN.source + " Working · waiting for the model · 0s · esc stop$"));
  expect(ticks()).toBe(Math.floor(300 / TICK_MS));
  time.advance(1700);
  expect(shown(activity, 185, time.now())).toContain("· 2s");
  // The turn ends: the done label stays for the notice time, then the clock stops by itself.
  activity.settled(false); clock.sync();
  expect(shown(activity, 185, time.now())).toMatch(/^✓ done · 2s$/);
  time.advance(NOTICE_MS + TICK_MS);
  expect(activity.indicator(185, time.now())).toEqual([]);
  expect(clock.running).toBeFalse();
  expect(time.timers.size).toBe(0);
});

test("a hold keeps the clock for a live row; one release per id; still motion ticks once a second and draws ●", () => {
  const { time, clock, ticks, still } = rig();
  const release = clock.hold("job:j1");
  expect(clock.hold("job:j1")).toBe(release);
  expect(clock.running).toBeTrue();
  still(true); setMotion("still");
  const before = ticks();
  time.advance(STILL_TICK_MS * 3);
  expect(ticks() - before).toBe(3);
  const activity = new Activity(time.now);
  activity.agentStart();
  const frame = shown(activity, 92, time.now());
  expect(frame.startsWith("● ")).toBeTrue();
  time.advance(STILL_TICK_MS);
  expect(shown(activity, 92, time.now())).toContain("1s");
  release();
  expect(clock.running).toBeFalse();
  expect(time.timers.size).toBe(0);
});

test("each phase reads as glyph and words: tools with their nested call, retry, compacting, owner, stall, stopped", () => {
  const { time, activity } = rig();
  activity.agentStart();
  activity.update("thinking_delta");
  expect(shown(activity, 92, time.now())).toMatch(/thinking/);
  activity.update("toolcall_delta", "eval_js");
  expect(shown(activity, 92, time.now())).toMatch(/writing JavaScript cell/);
  activity.messageEnd("toolUse");
  activity.toolStart("c1", "eval_js");
  activity.toolStart("c1/1", "read", "c1");
  time.advance(4000);
  expect(shown(activity, 92, time.now())).toMatch(new RegExp("^" + SPIN.source + " running JavaScript cell › read · 4s · esc stop$"));
  activity.toolEnd("c1");
  expect(activity.phase()).toBe("model");
  // A blocking dialog the agent opened shows no spinner; the owner's own palette does not count.
  activity.promptStart(true);
  expect(activity.phase()).toBe("model");
  activity.promptEnd();
  activity.promptStart(false);
  expect(shown(activity, 92, time.now())).toMatch(/^▲ waiting for you · 0s$/);
  activity.promptEnd();
  time.advance(STALL_MS);
  expect(shown(activity, 92, time.now())).toMatch(/^! no data from the model for 1m 00s · esc stop$/);
  activity.data();
  expect(shown(activity, 92, time.now())).not.toContain("no data");
  activity.messageEnd("error"); activity.agentEnd();
  expect(shown(activity, 92, time.now())).toContain("retrying after an error");
  activity.agentStart();
  activity.compactStart();
  expect(shown(activity, 92, time.now())).toContain("compacting context");
  activity.compactEnd();
  const end = activity.settled(true);
  expect(end?.aborted).toBeTrue();
  expect(shown(activity, 92, time.now())).toMatch(/^✗ stopped · /);
});

test("the indicator sits in the composer border at every tier and the receiver always survives", () => {
  const { time, activity } = rig();
  activity.agentStart(); activity.toolStart("b1", "bash");
  time.advance(64_000);
  const title = composerTitle(null, { label: "parent", color: PANEL_BLUE }), keep = visibleWidth(title);
  for (const columns of [185, 105, 92, 61, 46]) {
    const line = composerTop(title, columns, false, "", activity.indicator(columns, time.now(), "esc"), keep);
    const text = plain(line);
    expect(visibleWidth(line)).toBe(columns);
    expect(text).toContain("Message → parent");
    expect(text).toMatch(SPIN);
    expect(text).toMatch(/1m ?04s/);
    expect(text.includes("esc stop")).toBe(columns >= 92);
  }
});

test("the header state word carries the spinner and the turn time, and keeps its width", () => {
  const { time, activity } = rig();
  activity.agentStart();
  time.advance(42_000);
  const mark = activity.headerMark(time.now());
  for (const width of [185, 105, 92, 61, 46]) {
    const tiny = width < 64;
    const row = headerFrame({ title: "T-1 · R1", state: "Working", ...(mark ? { mark } : {}) }, width, { short: false, tiny }).lines[0]!;
    expect(visibleWidth(row)).toBe(width);
    const text = plain(row).trimEnd();
    expect(text).toMatch(new RegExp(SPIN.source + " [Ww]orking" + (tiny ? "$" : " · 42s$")));
  }
});

test("one snapshot per render pass: later reads reuse it until the pass ends or state changes", async () => {
  let builds = 0;
  const cache = new PassCache(() => ++builds);
  expect(cache.get()).toBe(1); expect(cache.get()).toBe(1);
  await Promise.resolve();
  expect(cache.get()).toBe(2);
  cache.clear();
  expect(cache.get()).toBe(3);
});
