import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionToolContext } from "@earendil-works/pi-coding-agent";
import { investigationTool } from "../src/roles/assistant/investigate.ts";

test("closed Git inspection ignores retargeting and disables executable diff/fsmonitor hooks and index locks", async () => {
  const root = mkdtempSync(join(tmpdir(), "planning-git-"));
  const previous = process.env.GIT_DIR;
  try {
    const git = (...args: string[]) => execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
    git("init", "-q"); git("config", "user.name", "test"); git("config", "user.email", "test@example.invalid");
    writeFileSync(join(root, "file.txt"), "before\n"); git("add", "file.txt"); git("commit", "-qm", "fixture");
    const head = git("rev-parse", "HEAD").trim();
    const marker = join(root, "executed");
    const script = join(root, "writer");
    writeFileSync(script, `#!/bin/sh\nprintf forbidden > '${marker}'\n`, { mode: 0o700 });
    git("config", "core.fsmonitor", script); git("config", "diff.external", script); git("config", "diff.evil.textconv", script);
    writeFileSync(join(root, ".gitattributes"), "file.txt diff=evil\n");
    writeFileSync(join(root, ".git/hooks/post-index-change"), readFileSync(script), { mode: 0o700 });
    const index = readFileSync(join(root, ".git/index"));
    writeFileSync(join(root, "file.txt"), "after\n");
    process.env.GIT_DIR = join(root, "absent-retarget");
    const tool = investigationTool(root);
    for (const fact of ["head", "status", "diff", "staged_diff"]) {
      const result = await tool.execute(fact, { fact }, undefined, undefined, {} as ExtensionToolContext);
      const text = result.content.map(p => p.type === "text" ? p.text : "").join("");
      if (fact === "head") expect(text.trim()).toBe(head);
      if (fact === "diff") expect(text).toContain("+after");
    }
    await expect(tool.execute("bad", { fact: "diff; run writer" }, undefined, undefined, {} as ExtensionToolContext)).rejects.toThrow("unknown investigation");
    expect(existsSync(marker)).toBeFalse();
    expect(readFileSync(join(root, ".git/index"))).toEqual(index);
  } finally { if (previous === undefined) delete process.env.GIT_DIR; else process.env.GIT_DIR = previous; rmSync(root, { recursive: true, force: true }); }
});
