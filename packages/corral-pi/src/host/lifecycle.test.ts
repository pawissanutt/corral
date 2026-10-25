import { afterAll, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { openSession } from "./lifecycle.ts";

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

  const resumed = openSession(sdk, cwd, sessions, 2);
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

test("a resumed generation with no earlier session starts a new one", async () => {
  const sdk = await import("@earendil-works/pi-coding-agent");
  const empty = join(work, "empty-sessions");
  mkdirSync(empty, { recursive: true });
  const resumed = openSession(sdk, cwd, empty, 3);
  expect(resumed.getSessionId()).not.toBe(LOST_SESSION);
  expect(resumed.getEntries().some((entry) => entry.type === "message")).toBeFalse();
});
