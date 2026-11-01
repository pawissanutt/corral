import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

test("release build runs the persistent Code Mode kernel, has no QuickJS path, and installs the Pi prompt's documentation paths", () => {
  const root = mkdtempSync(join(tmpdir(), "corral-pi-docs-"));
  try {
    const binary = join(root, "corral-pi-host");
    execFileSync(resolve(PACKAGE_ROOT, "../../scripts/build-pi-host.sh"), [binary], { stdio: "pipe" });
    // The persistent kernel's worker is embedded: no source checkout is reachable from root.
    const kernel = execFileSync(binary, ["--check-code-kernel"], { cwd: root, encoding: "utf8", timeout: 30_000 });
    expect(kernel).toMatch(/code kernel worker: file:\/\/\/\$bunfs\/root\/src\/extensions\/codemode\/kernel-worker\.js/);
    for (const proof of ["second cell", "queued cancel", "active cancel", "after cancel", "after reset", "blocked cancel",
      "after blocked cancel", "detached block", "after detached block"]) {
      expect(kernel).toContain(`code kernel ${proof}:`);
    }
    expect(kernel).toContain("code kernel check passed");
    const assets = join(root, "assets");
    execFileSync(binary, ["--write-install", assets]);
    for (const path of ["README.md", "docs/codemode.md", "docs/extensions.md", "docs/tui.md", "examples/extensions"]) {
      expect(existsSync(join(assets, path))).toBe(true);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}, 60_000);
