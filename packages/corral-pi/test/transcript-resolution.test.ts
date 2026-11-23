import { expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ChildSnapshot } from "../src/build/types.ts";
import { childRows } from "../src/ui/build/fleet-rows.ts";
import { RetainedChildren } from "../src/host/retained-children.ts";
import { mergeArchivedChildren, resolveChildRecording } from "../src/host/transcript-resolution.ts";

const header = (id: string) => `${JSON.stringify({ type: "session", id })}\n`;
function fixture(run: (root: string) => void) {
  const root = mkdtempSync(join(realpathSync(tmpdir()), "transcript-resolution-"));
  try { run(root); } finally { rmSync(root, { recursive: true, force: true }); }
}

test("exact retained Explore reads the recorded SDK header ID without a fleet roster", () => fixture(root => {
  const file = join(root, "runtime/auxiliary/request.jsonl"), id = "e-retained";
  mkdirSync(join(root, "runtime/auxiliary"), { recursive: true });
  mkdirSync(join(root, "child-records"));
  writeFileSync(file, header("request"));
  const index = join(root, `child-records/${id}.json`);
  const record = { session_file: "runtime/auxiliary/request.jsonl", session_id: "request", detail: { id, kind: "explore" } };
  writeFileSync(index, JSON.stringify(record));
  const retained = new RetainedChildren(root);
  expect(retained.recording(id)).toEqual({ file, session: "request" });
  expect(retained.recording("request")).toBeUndefined();
  writeFileSync(file, header("other-request"));
  expect(retained.recording(id)).toBeUndefined();
  writeFileSync(index, JSON.stringify({ ...record, session_id: undefined }));
  expect(retained.recording(id)).toBeUndefined();
  writeFileSync(file, header("request"));
  expect(retained.recording(id)).toEqual({ file, session: "request" });
  writeFileSync(index, JSON.stringify({ ...record, detail: { id, kind: "helper:web_search" } }));
  expect(retained.recording(id)).toBeUndefined();
}));

test("a known archived Build child uses its exact filename, never another parent's file or a link", () => fixture(root => {
  const parent = join(root, "parent"), other = join(root, "foreign");
  mkdirSync(join(parent, "children"), { recursive: true });
  mkdirSync(join(other, "children"), { recursive: true });
  const file = join(parent, "children/slice.jsonl"), foreign = join(other, "children/slice.jsonl");
  writeFileSync(file, header("build-request"));
  writeFileSync(foreign, header("build-request"));
  expect(resolveChildRecording(parent, "slice", { file })).toEqual({ file, session: "build-request" });
  expect(resolveChildRecording(parent, "slice", { file, session: "incorrect" })).toBeUndefined();
  expect(resolveChildRecording(parent, "different", { file })).toBeUndefined();
  expect(resolveChildRecording(parent, "slice", { file: foreign })).toBeUndefined();
  expect(resolveChildRecording(parent, "../slice", { file })).toBeUndefined();
  expect(resolveChildRecording(parent, "slice", { file: `${parent}/children/../children/slice.jsonl` })).toBeUndefined();
  rmSync(file);
  symlinkSync(foreign, file);
  expect(resolveChildRecording(parent, "slice", { file })).toBeUndefined();
  symlinkSync(join(other, "children"), join(parent, "auxiliary"));
  expect(resolveChildRecording(parent, "slice", { file: join(parent, "auxiliary/slice.jsonl"), session: "build-request" })).toBeUndefined();
}));

test("archived rows offer transcript only, and current IDs take precedence", () => {
  const child: ChildSnapshot = { id: "old", title: "Archived", brief: "Original prompt", kind: "slice", call: "call", state: "done",
    owns: [], after: [], profile: "default", model: "fixture/model", createdAt: 1, endedAt: 2, edits: [], steers: [], pulse: [],
    history: "d", messages: [], usage: { input: 0, output: 0, turns: 0 } };
  const current: ChildSnapshot = { ...child, id: "active", title: "Current", state: "running", sessionFile: "/fixture/active.jsonl" };
  const fleet = mergeArchivedChildren({ children: [current], mail: [], maxParallel: 1 }, [child, { ...child, id: current.id }])!;
  expect(fleet.children.map(row => row.title)).toEqual(["Archived", "Current"]);
  const rows = childRows(fleet, 3);
  expect(rows[0]?.capabilities).toEqual({ open: true, transcript: true });
  expect(rows[1]?.capabilities).toMatchObject({ open: true, transcript: true, cancel: true });
});


test("new retained Build references persist their actual header ID for restart", () => fixture(root => {
  const dir = join(root, "children"), file = join(dir, "slice.jsonl");
  mkdirSync(dir);
  writeFileSync(file, header("sdk-request"));
  const child: ChildSnapshot = { id: "slice", kind: "slice", title: "Child", brief: "Prompt", call: "call", profile: "default",
    model: "fixture/model", state: "done", createdAt: 1, owns: [], after: [], edits: [], usage: { input: 0, output: 0, turns: 0 },
    steers: [], pulse: [], history: "d", messages: [], sessionFile: file };
  expect(new RetainedChildren(root).child(child)).toBeTrue();
  expect(JSON.parse(readFileSync(join(root, "child-records/slice.json"), "utf8"))).toMatchObject({ session_id: "sdk-request" });
  const resumed = new RetainedChildren(root);
  expect(resumed.recording("slice")).toEqual({ file, session: "sdk-request" });
  writeFileSync(file, header("replaced"));
  expect(resumed.recording("slice")).toBeUndefined();
}));
