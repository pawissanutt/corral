import { expect, test } from "bun:test";
import { mkdtemp, readFile, rm, symlink, truncate, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import { getCodeCloneSeed, getCodeForkChoices, getCodeForkSeed, readCodeImportSeed } from "../src/host/code-copies.ts";

const user = (content: string) => ({ role: "user" as const, content, timestamp: 0 });
const stamp = "2025-01-01T00:00:00.000Z";
const header = { type: "session", version: 3, id: "old-session", cwd: "/old/project", timestamp: stamp, parentSession: "/old/parent" };
const line = (id: string, parentId: string | null, text: string) => ({
  type: "message", id, parentId, timestamp: stamp, message: user(text),
});

async function withJsonl(entries: unknown[], run: (file: string, original: string) => Promise<void>) {
  const dir = await mkdtemp(join(tmpdir(), "code-copies-"));
  try {
    const file = join(dir, "source.jsonl");
    const original = entries.map(value => JSON.stringify(value)).join("\n") + "\n";
    await writeFile(file, original);
    await run(file, original);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

test("fork offers user entries across branches and copies only the selected entry's ancestors", () => {
  const sessions = SessionManager.inMemory();
  const root = sessions.appendMessage(user("root"));
  const left = sessions.appendMessage(user("left"));
  const selected = sessions.appendMessage({ ...user(""), content: [
    { type: "text", text: "try " }, { type: "image", data: "aA==", mimeType: "image/png" }, { type: "text", text: "again" },
  ] });
  sessions.branch(root);
  const right = sessions.appendMessage(user("right"));
  const state = sessions.appendCustomEntry("corral_turn", 3);
  expect(getCodeForkChoices(sessions)).toEqual([
    { id: root, text: "root" }, { id: left, text: "left" }, { id: selected, text: "try again" }, { id: right, text: "right" },
  ]);
  const { seed, selectedText } = getCodeForkSeed(sessions, selected);
  expect(selectedText).toBe("try again");
  expect(seed.map(entry => entry.type === "message" && entry.message)).toEqual([user("root"), user("left")]);
  expect(seed[0]?.id).not.toBe(root);
  expect(seed[1]?.parentId).toBe(seed[0]?.id);
  expect(getCodeForkSeed(sessions, root).seed).toEqual([]);
  expect(() => getCodeForkSeed(sessions, state)).toThrow("Select a user message");
  expect(sessions.getLeafId()).toBe(state);
  expect(sessions.getEntry(selected)?.parentId).toBe(left);
});

test("clone follows the live leaf and removes runtime state while preserving compaction", () => {
  const sessions = SessionManager.inMemory();
  const root = sessions.appendMessage(user("old"));
  const state = sessions.appendCustomEntry("corral_job_control", { pid: 123 });
  const kept = sessions.appendMessage(user("kept"));
  sessions.appendCustomMessageEntry("corral_question", "question", true, { id: "old" });
  const leaf = sessions.appendCompaction("summary", state, 900, { pid: 123 });
  sessions.branch(root);
  sessions.appendMessage(user("inactive"));
  sessions.branch(leaf);
  const seed = getCodeCloneSeed(sessions);
  expect(seed.map(entry => entry.type)).toEqual(["message", "message", "compaction"]);
  expect(seed.at(-1)).toMatchObject({ firstKeptEntryId: seed[1]?.id, parentId: seed[1]?.id, summary: "summary" });
  expect(Object.hasOwn(seed.at(-1)!, "details")).toBe(false);
  expect(seed[1]?.id).not.toBe(kept);
  expect(sessions.getEntry(leaf)).toMatchObject({ firstKeptEntryId: state, details: { pid: 123 } });
});

test("import preserves the tree and active path without copying headers or runtime records", async () => {
  await withJsonl([header, line("root", null, "root"), line("left", "root", "left"), line("right", "root", "right"), {
    type: "custom", id: "old-job", parentId: "left", timestamp: stamp, customType: "corral_job_control", data: { pid: 123 },
  }], async (file, original) => {
    const seed = await readCodeImportSeed(file);
    expect(seed.map(entry => entry.type)).toEqual(["message", "message", "message", "session_info"]);
    expect(seed[0]?.id).not.toBe("root");
    expect(seed[0]?.parentId).toBeNull();
    expect(seed[1]?.parentId).toBe(seed[0]?.id);
    expect(seed[2]?.parentId).toBe(seed[0]?.id);
    expect(seed[3]?.parentId).toBe(seed[1]?.id);
    expect(await readFile(file, "utf8")).toBe(original);
  });
});

test("import migrates legacy entries in memory and rejects malformed ancestry", async () => {
  const { id, parentId, ...legacy } = line("old", null, "legacy");
  await withJsonl([{ ...header, version: 1 }, legacy], async (file, original) => {
    expect(await readCodeImportSeed(file)).toMatchObject([{ type: "message", parentId: null, message: user("legacy") }]);
    expect(await readFile(file, "utf8")).toBe(original);
  });
  await withJsonl([header, line(id, "missing", "broken")], async (file, original) => {
    await expect(readCodeImportSeed(file)).rejects.toThrow("invalid entry ancestry");
    expect(await readFile(file, "utf8")).toBe(original);
  });
});

test("import rejects symlinks to non-regular files and oversized files before parsing", async () => {
  await withJsonl([header], async file => {
    const device = `${file}.device`;
    await symlink("/dev/null", device);
    await expect(readCodeImportSeed(device)).rejects.toThrow("regular file");
    await truncate(file, 32 * 1024 * 1024 + 1);
    await expect(readCodeImportSeed(file)).rejects.toThrow("32 MiB limit");
  });
});
