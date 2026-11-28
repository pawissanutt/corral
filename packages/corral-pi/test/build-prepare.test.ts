import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Run } from "../src/build/corral-cli.ts";
import { rebaseCandidate } from "../src/build/prepare.ts";

test("a locked, unidentified Build rebase names the checkout and inspection without aborting", async () => {
  const cwd = mkdtempSync(join(tmpdir(), "build rebase lock-"));
  const dir = join(cwd, ".git"), rebase = join(dir, "rebase-merge"), lock = join(dir, "index.lock");
  const commands: string[] = [];
  const run: Run = async args => {
    const command = args.join(" ");
    commands.push(command);
    if (args[0] === "rev-parse") return { code: 0, stdout: `${dir}\n`, stderr: "" };
    if (args[0] === "-c") return { code: 1, stdout: "", stderr: "fatal: Unable to create '.git/index.lock': File exists." };
    if (args[0] === "diff") return { code: 0, stdout: "conflict.txt\0", stderr: "" };
    if (args[0] === "reflog") return { code: 0, stdout: "rebase (start): checkout main\n", stderr: "" };
    throw new Error(`Unexpected Git command: ${command}`);
  };
  try {
    mkdirSync(rebase, { recursive: true });
    writeFileSync(lock, "untouched lock");
    const failure = await rebaseCandidate(run, cwd, "base-tip", "source-tip").catch((error: Error) => error);
    expect(commands).not.toContain("rebase --abort");
    expect(existsSync(rebase)).toBe(true);
    expect(readFileSync(lock, "utf8")).toBe("untouched lock");
    expect(failure?.message).toContain(cwd);
    expect(failure?.message).toContain(lock);
    expect(failure?.message).toContain("rebase-merge");
    expect(failure?.message).toContain("did not abort");
    expect(failure?.message).toMatch(/cannot identify the lock holder/i);
    expect(failure?.message).toContain("git --no-optional-locks status");
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});
