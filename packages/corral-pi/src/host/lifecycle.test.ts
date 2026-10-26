import { afterAll, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { openSession, SessionResumeRefused } from "./lifecycle.ts";

const work = mkdtempSync(join(tmpdir(), "corral-pi-lifecycle-"));
const cwd = join(work, "worktree");
const sessions = join(work, "sessions");
mkdirSync(cwd, { recursive: true });
mkdirSync(sessions, { recursive: true });
const fixture = fileURLToPath(new URL("../../../../tools/pi-probe/fixtures/session-dangling.jsonl", import.meta.url));
const LOST_SESSION = "01a167ce-69e8-72a2-ae3c-efc763d16640";

afterAll(() => rmSync(work, { recursive: true, force: true }));

function rehomedDanglingFixture(): string {
  const lines = readFileSync(fixture, "utf8").trim().split("\n").map((line) => {
    const entry = JSON.parse(line) as Record<string, unknown>;
    if (entry.type === "session") entry.cwd = cwd;
    return JSON.stringify(entry);
  });
  const path = join(sessions, `${LOST_SESSION}.jsonl`);
  writeFileSync(path, `${lines.join("\n")}\n`);
  return path;
}

test("a resumed generation continues the lost generation's session; generation 1 starts fresh", async () => {
  const sdk = await import("@earendil-works/pi-coding-agent");
  const path = rehomedDanglingFixture();
  const before = readFileSync(path, "utf8");

  const resumed = openSession(sdk, cwd, sessions, 2, path);
  expect(resumed.getSessionId()).toBe(LOST_SESSION);
  expect(resumed.getSessionFile()).toBe(path);
  const calls = resumed.getEntries().flatMap((entry) =>
    entry.type === "message" && entry.message.role === "assistant"
      ? entry.message.content.filter((item) => item.type === "toolCall").map((item) => item.name)
      : []);
  expect(calls).toEqual(["corral_gate", "edit"]);
  expect(readFileSync(path, "utf8")).toBe(before);

  const fresh = openSession(sdk, cwd, sessions, 1);
  expect(fresh.getSessionId()).not.toBe(LOST_SESSION);
  expect(fresh.getEntries().some((entry) => entry.type === "message")).toBeFalse();
});

test("a resumed generation without an exact session refuses context loss", async () => {
  const sdk = await import("@earendil-works/pi-coding-agent");
  const empty = join(work, "empty-sessions");
  mkdirSync(empty, { recursive: true });
  expect(() => openSession(sdk, cwd, empty, 3)).toThrow(SessionResumeRefused);
});

test("review-child generation 1 and host recovery open the exact file, never a newer sibling", async () => {
  const sdk = await import("@earendil-works/pi-coding-agent");
  const path = rehomedDanglingFixture();
  const before = readFileSync(path, "utf8");
  const decoyId = "01a167ce-69e8-72a2-ae3c-efc763d16641";
  const decoy = join(sessions, "newer.jsonl");
  writeFileSync(decoy, before.replaceAll(LOST_SESSION, decoyId));
  utimesSync(path, new Date("2026-01-01"), new Date("2026-01-01"));
  utimesSync(decoy, new Date("2026-10-26"), new Date("2026-10-26"));
  const childCwd = join(work, "review-child");
  mkdirSync(childCwd);
  for (const generation of [1, 2, 3]) {
    const resumed = openSession(sdk, childCwd, sessions, generation, path);
    expect(resumed.getSessionId()).toBe(LOST_SESSION);
    expect(resumed.getSessionFile()).toBe(path);
    expect(resumed.getSessionDir()).toBe(sessions);
    expect(resumed.getCwd()).toBe(childCwd);
    expect(resumed.getEntries().some((entry) => entry.type === "message")).toBeTrue();
    expect(readFileSync(path, "utf8")).toBe(before);
  }
});

test("an exact resume refuses a missing, empty or incompatible file instead of creating or choosing another", async () => {
  const sdk = await import("@earendil-works/pi-coding-agent");
  rehomedDanglingFixture();
  const missing = join(sessions, "missing.jsonl");
  for (const generation of [1, 2]) {
    expect(() => openSession(sdk, cwd, sessions, generation, missing)).toThrow(SessionResumeRefused);
    expect(existsSync(missing)).toBeFalse();
  }
  for (const [name, text] of [
    ["empty", ""],
    ["invalid", "not a Pi session\n"],
    ["future", `${JSON.stringify({ type: "session", id: LOST_SESSION, version: sdk.CURRENT_SESSION_VERSION + 1, cwd })}\n`],
  ]) {
    const path = join(sessions, `${name}.jsonl`);
    writeFileSync(path, text ?? "");
    expect(() => openSession(sdk, cwd, sessions, 1, path)).toThrow(SessionResumeRefused);
    expect(readFileSync(path, "utf8")).toBe(text);
  }
});

test("an explicit fresh generation ignores prior sessions even when recovering an existing host", async () => {
  const sdk = await import("@earendil-works/pi-coding-agent");
  const path = rehomedDanglingFixture();
  const before = readFileSync(path, "utf8");
  const fresh = openSession(sdk, cwd, sessions, 2, path, true);
  expect(fresh.getSessionId()).not.toBe(LOST_SESSION);
  expect(fresh.getSessionFile()).not.toBe(path);
  expect(fresh.getEntries().some((entry) => entry.type === "message")).toBeFalse();
  expect(readFileSync(path, "utf8")).toBe(before);
});
