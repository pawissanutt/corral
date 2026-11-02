import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CorralClient } from "./corral.ts";

test("canonical inbox reader pins the launch root and fixed argv, closes stdin and never sends host secrets", async () => {
  const root = mkdtempSync(join(tmpdir(), "inbox-read-"));
  const executable = join(root, "reader");
  writeFileSync(executable, '#!/usr/bin/env bun\nprocess.stdout.write(JSON.stringify({args:process.argv.slice(2),cwd:process.cwd(),stdin:await Bun.stdin.text()}));\n', { mode: 0o700 });
  const client = new CorralClient({ corral: executable, root, identity: { store: "s", project: "p", role: "worker", task: "T-1", run: "R1", generation: 1 }, hostCap: "not-for-the-reader", sessionId: () => "not-for-the-reader" });
  try {
    expect(await client.attentionSnapshot()).toEqual({ args: ["--root", root, "attention", "inbox", "--json"], cwd: root, stdin: "" });
    const abort = new AbortController(); abort.abort();
    await expect(client.attentionSnapshot(abort.signal)).rejects.toThrow();
  } finally { rmSync(root, { recursive: true, force: true }); }
});
