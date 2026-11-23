import { expect, test } from "bun:test";
import { appendFileSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readTranscriptHeader, readTranscriptPage } from "../src/host/transcript-preview.ts";
import { TranscriptBytes } from "../src/host/transcript-pages.ts";

test("reader validates identity, bounds bytes/lines and never rewrites or revives", async () => {
  const dir = mkdtempSync(join(tmpdir(), "transcript-preview-")), file = join(dir, "session.jsonl");
  try {
    const source = JSON.stringify({ type: "session", version: 1, id: "existing" }) + "\n" + ('{"type":"message","text":"' + "x".repeat(200) + '"}\n').repeat(300);
    writeFileSync(file, source); const before = statSync(file);
    const read = (path = file, session = "existing") => readTranscriptPage(path, session, { mode: "raw" });
    const page = await read();
    expect(Buffer.byteLength(page.rows.map(r => r.text).join(""))).toBeLessThanOrEqual(24 * 1024);
    expect(page.rows.length).toBeLessThanOrEqual(32);
    expect(page.newer).toBeDefined();
    expect(readFileSync(file, "utf8")).toBe(source); expect(statSync(file).mtimeMs).toBe(before.mtimeMs);
    expect(readTranscriptHeader(file)).toBe("existing");
    await expect(read(file, "foreign")).rejects.toThrow("identity mismatch");
    symlinkSync(file, join(dir, "link")); await expect(read(join(dir, "link"))).rejects.toThrow();
    expect(readTranscriptHeader(join(dir, "link"))).toBeUndefined();
    await expect(read(dir)).rejects.toThrow("regular file");
    await expect(read(join(dir, "missing"))).rejects.toThrow();
    await expect(read("/dev/null")).rejects.toThrow("regular file");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("retained history reaches both ends across forks and large UTF-8 results", async () => {
  const dir = mkdtempSync(join(tmpdir(), "transcript-history-")), file = join(dir, "session.jsonl");
  const result = "é🪶".repeat(12000);
  const entries: object[] = [{ type: "session", id: "existing" }];
  for (let i = 0; i <= 250; i++) entries.push({ type: "message", id: `m${i}`, parentId: i ? `m${i - 1}` : null, message: { role: "user", content: `marker-${i};` } });
  entries.push({ type: "message", id: "fork", parentId: "m0", message: { role: "assistant", content: "abandoned branch" } });
  entries.push({ type: "message", id: "tip", parentId: "m250", message: { role: "toolResult", content: [{ type: "text", text: result }] } });
  const source = entries.map(e => JSON.stringify(e) + "\n").join("");
  try {
    writeFileSync(file, source);
    let conversation = "", raw = "";
    let page = await readTranscriptPage(file, "existing", { mode: "conversation" });
    const first = page.rows;
    const snapshots = [first];
    for (;;) {
      conversation = page.rows.map(r => r.text).join("") + conversation;
      if (!page.older) break;
      const previous = page;
      page = await readTranscriptPage(file, "existing", { mode: "conversation", cursor: page.older });
      snapshots.push(page.rows);
      expect(Buffer.byteLength(page.rows.map(r => r.text).join(""))).toBeLessThanOrEqual(24 * 1024);
      expect(page.newer).toBeDefined();
      expect((await readTranscriptPage(file, "existing", { mode: "conversation", cursor: page.newer })).rows).toEqual(previous.rows);
    }
    expect(conversation).toBe(Array.from({ length: 251 }, (_, i) => `marker-${i};`).join("") + result);
    for (let i = snapshots.length - 2; i >= 0; i--) {
      expect(page.newer).toBeDefined();
      page = await readTranscriptPage(file, "existing", { mode: "conversation", cursor: page.newer });
      expect(page.rows).toEqual(snapshots[i]!);
    }
    expect(page.newer).toBeUndefined();
    page = await readTranscriptPage(file, "existing", { mode: "raw" });
    for (;;) {
      const previous = page;
      raw += page.rows.map(r => r.text).join("");
      if (!page.newer) break;
      page = await readTranscriptPage(file, "existing", { mode: "raw", cursor: page.newer });
      expect((await readTranscriptPage(file, "existing", { mode: "raw", cursor: page.older })).rows).toEqual(previous.rows);
    }
    expect(raw).toBe(source);
    expect(first.length).toBeGreaterThan(0);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("header-only sessions are empty; unfinished appends wait and changed reads fail", async () => {
  const dir = mkdtempSync(join(tmpdir(), "transcript-change-")), file = join(dir, "session.jsonl");
  const header = '{"type":"session","id":"existing"}\n';
  const read = (cursor?: string) => readTranscriptPage(file, "existing", { mode: "raw", cursor });
  try {
    writeFileSync(file, header);
    expect((await readTranscriptPage(file, "existing", { mode: "conversation" })).rows).toEqual([]);
    appendFileSync(file, "{unfinished");
    expect((await read()).rows.map(r => r.text).join("")).toBe(header);
    appendFileSync(file, "\n" + "{}\n".repeat(40));
    const malformed = await readTranscriptPage(file, "existing", { mode: "conversation" });
    expect(malformed.rows[0]?.text).toContain("Malformed transcript record");
    const { newer } = await read();
    expect(newer).toBeDefined();
    await expect(read(`${newer}x`)).rejects.toThrow("Invalid transcript cursor");
    await expect(readTranscriptPage(file, "existing", { mode: "conversation", cursor: newer })).rejects.toThrow("cursor mode");
    const original = TranscriptBytes.prototype.read;
    let mutated = false;
    TranscriptBytes.prototype.read = async function (start, length) {
      const data = await original.call(this, start, length);
      if (!mutated) {
        mutated = true;
        appendFileSync(file, "{}\n");
      }
      return data;
    };
    try { await expect(read(newer)).rejects.toThrow("Transcript changed"); }
    finally { TranscriptBytes.prototype.read = original; }
    await expect(read(newer)).rejects.toThrow("Transcript changed");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
