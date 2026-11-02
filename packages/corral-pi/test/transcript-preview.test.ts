import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { transcriptPreview } from "../src/host/transcript-preview.ts";

test("existing transcript preview validates identity, bounds bytes/lines and never rewrites or revives", () => {
  const dir = mkdtempSync(join(tmpdir(), "transcript-preview-")), file = join(dir, "session.jsonl");
  try {
    const source = JSON.stringify({ type: "session", version: 1, id: "existing" }) + "\n" + ('{"type":"message","text":"' + "x".repeat(200) + '"}\n').repeat(300);
    writeFileSync(file, source); const before = statSync(file);
    const result = transcriptPreview(file, "existing");
    expect(Buffer.byteLength(result) < 25 * 1024).toBeTrue(); expect(result).toContain("[preview truncated]");
    expect(result.split("\n").length).toBeLessThanOrEqual(202);
    expect(readFileSync(file, "utf8")).toBe(source); expect(statSync(file).mtimeMs).toBe(before.mtimeMs);
    expect(() => transcriptPreview(file, "foreign")).toThrow("identity mismatch");
    symlinkSync(file, join(dir, "link")); expect(() => transcriptPreview(join(dir, "link"), "existing")).toThrow();
    expect(() => transcriptPreview(dir, "existing")).toThrow("regular file");
    expect(() => transcriptPreview(join(dir, "missing"), "existing")).toThrow();
    expect(() => transcriptPreview("/dev/null", "existing")).toThrow("regular file");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
