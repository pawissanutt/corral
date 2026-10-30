import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

test("release build runs nested codemode and installs the Pi prompt's documentation paths", () => {
  const root = mkdtempSync(join(tmpdir(), "corral-pi-docs-"));
  try {
    const binary = join(root, "corral-pi-host");
    execFileSync(resolve(PACKAGE_ROOT, "../../scripts/build-pi-host.sh"), [binary], { stdio: "pipe" });
    const output = execFileSync(binary, ["--check-codemode"], { cwd: root, encoding: "utf8" });
    expect(output).toContain("codemode nested ls:");
    expect(output).toContain("corral-pi-host");
    const assets = join(root, "assets");
    execFileSync(binary, ["--write-install", assets]);
    for (const path of ["README.md", "docs/codemode.md", "docs/extensions.md", "docs/tui.md", "examples/extensions"]) {
      expect(existsSync(join(assets, path))).toBe(true);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}, 60_000);
