import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SESSION_PART_MAX } from "../src/bridge/protocol.ts";
import { changesPage, RemoteChanges, type ChangedFile } from "../src/host/remote/changes.ts";
import { RemoteRegistry } from "../src/host/remote/registry.ts";
import { memoryBases } from "../src/ui/review-store.ts";

test("a changes page never splits a file and stops at 500 files", () => {
  const files: ChangedFile[] = Array.from({ length: 507 }, (_, i) => ({ path: `${i}/${'a\\"🔎'.repeat(65)}`,
    original_path: `${i}/before`, status: "R", added: 1, removed: 2, authors: ["agent"], outside: true }));
  const source = { files, comparison: "all" as const, root: "/test", comparisons: [], scopes: [], total: files.length,
    added: files.length, removed: 2 * files.length, truncated: false };
  let after: string | undefined;
  const seen: ChangedFile[] = [];
  do {
    const page = changesPage(source, after);
    expect(page.files.length).toBeGreaterThan(0);
    expect(page.files.length).toBeLessThanOrEqual(200);
    expect(page.total).toBe(507);
    expect(page.truncated).toBeTrue();
    expect(Buffer.byteLength(JSON.stringify(page))).toBeLessThanOrEqual(SESSION_PART_MAX);
    expect(Buffer.byteLength(page.next ?? "")).toBeLessThanOrEqual(128);
    seen.push(...page.files); after = page.next;
  } while (after !== undefined);
  expect(seen).toEqual(files.slice(0, 500));
});

test("a patch over the read limit is marked truncated", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-changes-"));
  const remote = new RemoteChanges({ cwd: dir, defaultComparison: "session", bases: memoryBases(), children: () => [], changed() {} });
  try {
    execFileSync("git", ["init", "-q", dir]);
    const path = "large.txt";
    writeFileSync(join(dir, path), "0123456789abcdef\n".repeat(18_000));
    const registry = new RemoteRegistry();
    remote.register(registry);
    const read = registry.get("change_file")!;
    const page = await read({ comparison: "all", path }) as { path: string; patch: string; truncated: boolean; next?: string };
    expect(page.path).toBe(path);
    expect(page.truncated).toBeTrue();
    expect(page.patch.length).toBeGreaterThan(0);
    expect(Buffer.byteLength(JSON.stringify(page))).toBeLessThanOrEqual(SESSION_PART_MAX);
    expect(page.next).toBeDefined();
    expect(await read({ comparison: "all", path, after: page.next })).toMatchObject({ path, truncated: true });
  } finally { remote.close(); rmSync(dir, { recursive: true, force: true }); }
});
