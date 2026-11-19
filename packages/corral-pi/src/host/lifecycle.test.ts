import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { openSession, SessionResumeRefused } from "./lifecycle.ts";

test("opening the recorded Pi transcript rejects missing or unsupported headers before touching data", async () => {
  const sdk = await import("@earendil-works/pi-coding-agent");
  const cwd = mkdtempSync(join(tmpdir(), "corral-pi-transcript-"));
  try {
    const sessions = join(cwd, "sessions");
    mkdirSync(sessions);
    const path = join(sessions, "exact.jsonl");
    expect(() => openSession(sdk, cwd, sessions, path)).toThrow(SessionResumeRefused);
    for (const value of ["", '{"partial":', JSON.stringify({ type: "session", id: "fixture", version: sdk.CURRENT_SESSION_VERSION + 1 }) + "\n"]) {
      writeFileSync(path, value);
      expect(() => openSession(sdk, cwd, sessions, path)).toThrow(SessionResumeRefused);
      expect(readFileSync(path, "utf8")).toBe(value);
    }
  } finally { rmSync(cwd, { recursive: true, force: true }); }
});
