import { expect, test } from "bun:test";
import { execFile } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

const exec = promisify(execFile);

test("compiled codemode runs cells on the embedded Corral kernel worker, including reload", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-codemode-binary-"));
  try {
    const binary = join(root, "codemode-smoke");
    await exec(process.execPath, [
      "build", "--compile", "--root", ".", "--no-compile-autoload-dotenv", "--no-compile-autoload-bunfig",
      "test/fixtures/compiled-codemode.ts", "src/extensions/codemode/kernel-worker.ts", "--outfile", binary,
    ], { cwd: PACKAGE_ROOT, timeout: 30_000 });
    const { stdout, stderr } = await exec(binary, [], {
      cwd: root, timeout: 15_000, env: { ...process.env, HOME: root, XDG_DATA_HOME: root },
    });
    expect(stderr).toBe("");
    expect(stdout).toContain("COMPILED_CODEMODE startup passed");
    expect(stdout).toContain("COMPILED_CODEMODE reload passed");
    expect(stdout).toMatch(/COMPILED_CODEMODE kernel file:\/\/\/\$bunfs\/root\/src\/extensions\/codemode\/kernel-worker\.js/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}, 60_000);
