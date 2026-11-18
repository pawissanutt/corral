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
    await expect(client.artifactFile("--help")).rejects.toThrow("Invalid artifact reference");
    expect(engine.calls()).toEqual([
      ["--root", root, "--json", "done", "T-1", "--summary", "try again"],
      ["--root", root, "artifact", "show", "T-1:2"],
    ]);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
