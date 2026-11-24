import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CorralClient, CorralCommandError } from "../src/tools/corral.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";

test("engine argv preserves corrective commands and uses artifact references directly", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-commands-"));
  try {
    const diagnostic = 'refused: the gate is red\nnext: corral done T-1 --force --summary "try again"';
    const bytes = [0xff, 0, 0x80, 0x0a];
    const engine = writeFakeEngine(root, { done: { stderr: diagnostic, code: 2 }, "artifact show": { bytes } });
    const client = new CorralClient({ corral: engine.path, root, cwd: root, sessionDir: root,
      identity: { store: "local", project: "", role: "worker", task: "T-1", run: "R1" } });
    try {
      await client.command(["done", "T-1", "--summary", "try again"]);
      throw new Error("expected refused command");
    } catch (error) {
      expect(error instanceof CorralCommandError).toBeTrue();
      expect(error).toMatchObject({ message: diagnostic, reason: "the gate is red", next: ['corral done T-1 --force --summary "try again"'] });
    }
    const path = await client.artifactFile("T-1:2");
    expect(readFileSync(path)).toEqual(Buffer.from(bytes));
    expect(statSync(path).mode & 0o777).toBe(0o600);
    const library = await client.artifactFile("-../project name:1");
    expect(readFileSync(library)).toEqual(Buffer.from(bytes));
    expect(library.startsWith(join(root, "artifact-previews") + "/")).toBeTrue();
    expect(library).not.toContain("project name");
    for (const invalid of ["--help", " :1", "demo:0", "demo:4294967296", "demo:1:2"]) {
      await expect(client.artifactFile(invalid)).rejects.toThrow("Invalid artifact reference");
    }
    expect(engine.calls()).toEqual([
      ["--root", root, "--json", "done", "T-1", "--summary", "try again"],
      ["--root", root, "artifact", "show", "--", "T-1:2"],
      ["--root", root, "artifact", "show", "--", "-../project name:1"],
    ]);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("usage can be reported after the task worktree is gone", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-usage-"));
  try {
    const engine = writeFakeEngine(root);
    const identity = { store: "local", project: "demo", role: "reviewer" as const, task: "T-1", run: "R2" };
    const client = new CorralClient({ corral: engine.path, root, cwd: join(root, "removed"), identity });
    await client.usage({ model: "fixture/model", input: 10, output: 2, cache_read: 0, cache_write: 0 });
    expect(engine.calls()).toEqual([["--root", root, "--json", "usage", "add", "--model", "fixture/model",
      "--task", "T-1", "--run", "R2", "--input", "10", "--output", "2", "--cache-read", "0", "--cache-write", "0"]]);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
