import { expect, test } from "bun:test";
import { execFile } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

const exec = promisify(execFile);

test("standalone compiled startup derives OAuth for both subscription providers offline", async () => {
  // Given the production compiled entrypoint bundled without a source tree at runtime.
  const root = mkdtempSync(join(tmpdir(), "corral-compiled-oauth-"));
  try {
    const binary = join(root, "oauth-smoke");
    await exec(process.execPath, [
      "build", "--compile", "--no-compile-autoload-dotenv", "--no-compile-autoload-bunfig",
      "test/fixtures/compiled-oauth.ts", "--outfile", binary,
    ], { cwd: PACKAGE_ROOT, timeout: 30_000 });
    // When the executable derives auth using synthetic unexpired credentials.
    const { stdout, stderr } = await exec(binary, [], {
      cwd: root, timeout: 10_000,
      env: { ...process.env, HOME: root, XDG_DATA_HOME: root },
    });
    // Then neither flow tries to import a missing relative module from /$bunfs.
    expect(stderr).toBe("");
    const results = stdout.split("\n").filter((line) => line.startsWith("{")).map((line) => JSON.parse(line));
    expect(results).toEqual([
      { provider: "openai-codex", derived: true }, { provider: "anthropic", derived: true },
    ]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}, 60_000);
