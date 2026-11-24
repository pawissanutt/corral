import { expect, test } from "bun:test";
import { appendFileSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readTranscriptHeader, readTranscriptPage } from "../src/host/transcript-preview.ts";
import { TranscriptBytes } from "../src/host/transcript-pages.ts";
import { toolMessageId } from "../src/extensions/subagent/inspection.ts";

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
  for (let i = 0; i < 65; i++) entries.push({ type: "message", id: `fork${i}`, parentId: i ? `fork${i-1}` : "m0", message: { role: "assistant", content: "abandoned branch" } });
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
    expect(snapshots.some(rows => !rows.length)).toBeTrue(); // A scan-only page crossing the abandoned branch.
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

test("repeated deep reverse clicks read only the requested page in both modes", async () => {
  const dir = mkdtempSync(join(tmpdir(), "transcript-reverse-")), file = join(dir, "session.jsonl");
  const original = TranscriptBytes.prototype.read;
  let reads = 0;
  try {
    writeFileSync(file, '{"type":"session","id":"existing"}\n' + Array.from({ length: 32 * 64 }, (_, i) =>
      JSON.stringify({ type: "message", message: { role: "user", content: `row-${i}` } }) + "\n").join(""));
    TranscriptBytes.prototype.read = function (start, length) { reads++; return original.call(this, start, length); };
    for (const mode of ["raw", "conversation"] as const) {
      const read = (cursor?: string) => readTranscriptPage(file, "existing", { mode, cursor });
      const forward = mode === "raw" ? "newer" : "older", reverse = mode === "raw" ? "older" : "newer";
      let page = await read();
      const firstIds = [page.rows[0]!.id];
      while (page[forward]) {
        page = await read(page[forward]);
        firstIds.push(page.rows[0]!.id);
      }
      expect(firstIds.length).toBeGreaterThanOrEqual(64);
      for (let i = firstIds.length - 2; i >= 0; i--) {
        expect(page[reverse]).toBeDefined();
        reads = 0;
        page = await read(page[reverse]);
        expect(page.rows[0]!.id).toBe(firstIds[i]!);
        // Header plus one page: raw data + continuation probe, or up to
        // 32 decoded records and a boundary window (two at a window edge).
        expect(reads).toBeLessThanOrEqual(mode === "raw" ? 3 : 35);
      }
      expect(page[reverse]).toBeUndefined();
    }
  } finally { TranscriptBytes.prototype.read = original; rmSync(dir, { recursive: true, force: true }); }
});

test("evicted snapshots require loading latest, even after the same snapshot is reopened", async () => {
  const dir = mkdtempSync(join(tmpdir(), "transcript-evict-"));
  try {
    const source = '{"type":"session","id":"existing"}\n' + "{}\n".repeat(40);
    const read = (file: string, cursor?: string) => readTranscriptPage(file, "existing", { mode: "raw", cursor });
    const file = join(dir, "first");
    writeFileSync(file, source);
    const cursor = (await read(file)).newer!;
    for (let i = 0; i < 16; i++) {
      const other = join(dir, String(i));
      writeFileSync(other, source);
      await read(other);
    }
    await expect(read(file, cursor)).rejects.toThrow("load latest");
    const latest = await read(file);
    await expect(read(file, cursor)).rejects.toThrow("load latest");
    expect((await read(file, latest.newer)).older).toBeDefined();
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

test("conversation rows name calls, results, stops and the slices of a split item; the system prompt is not a turn", async () => {
  const dir = mkdtempSync(join(tmpdir(), "transcript-rows-")), file = join(dir, "session.jsonl");
  const output = "r".repeat(30000);
  const entries = [
    { type: "session", id: "existing" },
    { type: "message", id: "s", parentId: null, message: { role: "system", content: "SYSTEM_PROMPT" } },
    { type: "message", id: "u", parentId: "s", message: { role: "user", content: [{ type: "text", text: "ask" }] } },
    { type: "message", id: "a", parentId: "u", message: { role: "assistant", stopReason: "toolUse", content: [
      { type: "thinking", thinking: "plan" }, { type: "text", text: "reading" }, { type: "toolCall", id: "c1", name: "read", arguments: { path: "a.ts" } }] } },
    { type: "message", id: "r", parentId: "a", message: { role: "toolResult", toolCallId: "c1", toolName: "read", isError: true, content: [{ type: "text", text: output }] } },
    { type: "message", id: "b", parentId: "r", message: { role: "assistant", stopReason: "toolUse", content: [{ type: "toolCall", id: "c2", name: "bash", arguments: {} }] } },
    { type: "message", id: "q", parentId: "b", message: { role: "toolResult", toolCallId: "c2", toolName: "bash", content: [] } },
    { type: "message", id: "e", parentId: "q", message: { role: "assistant", stopReason: "error", errorMessage: "overloaded", content: [] } },
  ];
  try {
    writeFileSync(file, entries.map(entry => JSON.stringify(entry) + "\n").join(""));
    let page = await readTranscriptPage(file, "existing", { mode: "conversation" });
    let rows = [...page.rows];
    while (page.older) {
      page = await readTranscriptPage(file, "existing", { mode: "conversation", cursor: page.older });
      rows = [...page.rows, ...rows];
    }
    // A call and its result share a fixed-size key of the call's id; a result without content still settles its call.
    const slices = rows.filter(row => row.kind === "tool" && row.callId === toolMessageId("c1"));
    expect(rows.map(row => row.kind)).toEqual(["user", "thinking", "assistant", "call", ...slices.map(() => "tool" as const), "call", "tool", "stop"]);
    expect(rows.find(row => row.kind === "call")).toMatchObject({ tool: "read", callId: toolMessageId("c1"), text: '{"path":"a.ts"}' });
    expect(rows.at(-2)).toMatchObject({ kind: "tool", tool: "bash", callId: toolMessageId("c2"), text: "" });
    expect(rows.at(-1)).toMatchObject({ stop: "error", text: "overloaded" });
    // One result over two pages: every slice names the same record and item, and says where it stands in it.
    expect(slices.length).toBeGreaterThan(1);
    expect(slices.map(row => row.text).join("")).toBe(output);
    expect(new Set(slices.map(row => `${row.record}:${row.item}:${row.callId}:${row.tool}:${row.error}`)).size).toBe(1);
    expect(slices.map(row => row.from ?? 0)).toEqual(slices.map((_, index) => slices.slice(0, index).reduce((sum, row) => sum + row.text.length, 0)));
    expect(slices.map(row => row.more === true)).toEqual(slices.map((_, index) => index < slices.length - 1));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
