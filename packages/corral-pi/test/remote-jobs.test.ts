import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SESSION_PART_MAX } from "../src/bridge/protocol.ts";
import { readOutput } from "../src/extensions/jobs/output.ts";
import { jobOutputPage, jobsPart, type ReadJobOutput } from "../src/host/remote/jobs.ts";

test("job output pages walk to older bytes and never exceed the bound", () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-output-"));
  const id = "j1", file = join(dir, "output"), text = "a\0\"🔎\n".repeat(14_000) + "newest\n";
  const data = Buffer.from(text), { length: bytes } = data;
  const read: ReadJobOutput = (job, options) => ({ bytes_total: bytes, bytes_stored: bytes, state: "exited",
    ...readOutput({ id: job, file, bytes, stored: bytes, tail: data.subarray(-64 * 1024), state: "exited" }, options) });
  try {
    writeFileSync(file, data);
    const pages: string[] = [];
    let after: string | undefined, end = bytes;
    do {
      const page = jobOutputPage(read, id, after);
      expect(page.bytes).toBe(bytes);
      expect(page.offset < end).toBe(true);
      expect(page.offset + Buffer.byteLength(page.text)).toBe(end);
      expect(Buffer.byteLength(page.text)).toBeLessThanOrEqual(40 * 1024);
      expect(Buffer.byteLength(JSON.stringify(page))).toBeLessThanOrEqual(SESSION_PART_MAX);
      expect(Buffer.byteLength(page.next ?? "")).toBeLessThanOrEqual(128);
      pages.push(page.text); end = page.offset; after = page.next;
    } while (after !== undefined);
    expect(pages.length).toBeGreaterThan(2);
    expect(pages.reverse().join("")).toBe(text);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("a completed job never advertises stop or background", () => {
  const runtime = { output: () => ({}), recovered: () => [], inspection: () => ([{ job: "j1", state: "exited" as const,
    command: "echo done", startedAt: 2000, endedAt: 3000, exitCode: 0, signal: null, groupClear: true,
    bytes: 5, truncated: false, tail: "done\n", monitorAttempts: 0, watches: [] }]) };
  expect(jobsPart(runtime, () => true).jobs[0]).toMatchObject({ id: "j1", exit_code: 0, tail: ["done"], actions: [] });
});
