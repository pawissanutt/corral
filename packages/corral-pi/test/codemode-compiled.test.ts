import { afterAll, expect, test } from "bun:test";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

interface Exit { readonly code: number | null; readonly signal: NodeJS.Signals | null; readonly stdout: string; readonly stderr: string }

/**
 * Resolves on the child's exit event with its output. Only a hang guard bounds it: on expiry the
 * child is killed and the wait fails, so host load slows the wait but never kills a working child (T-794).
 */
function exited(file: string, args: readonly string[], options: { cwd: string; env?: NodeJS.ProcessEnv }, guardMs: number): Promise<Exit> {
  return new Promise((resolve, reject) => {
    const child = spawn(file, args, { ...options, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => { stdout += chunk; });
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => { stderr += chunk; });
    const guard = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`${file} ${args.join(" ")} still running after the ${guardMs} ms hang guard\n${stderr}`));
    }, guardMs);
    child.once("error", (error) => { clearTimeout(guard); reject(error); });
    child.once("close", (code, signal) => { clearTimeout(guard); resolve({ code, signal, stdout, stderr }); });
  });
}

// Compile once at load, outside the timed test, like the browser warm-up: under host load the
// compile alone outlasted a fixed bound (T-794), so it is awaited to its exit with only a hang guard.
const root = mkdtempSync(join(tmpdir(), "corral-codemode-binary-"));
const binary = join(root, "codemode-smoke");
const build = await exited(process.execPath, [
  "build", "--compile", "--root", ".", "--no-compile-autoload-dotenv", "--no-compile-autoload-bunfig",
  "test/fixtures/compiled-codemode.ts", "src/extensions/codemode/kernel-worker.ts", "--outfile", binary,
], { cwd: PACKAGE_ROOT }, 600_000);

afterAll(() => rmSync(root, { recursive: true, force: true }));

test("compiled codemode runs cells on the embedded Corral kernel worker, including reload", async () => {
  if (build.code !== 0 || build.signal) throw new Error(`bun build failed (${build.signal ?? build.code})\n${build.stderr}`);
  const { code, signal, stdout, stderr } = await exited(binary, [], {
    cwd: root, env: { ...process.env, HOME: root, XDG_DATA_HOME: root },
  }, 55_000);
  expect([code, signal, stderr]).toEqual([0, null, ""]);
  expect(stdout).toContain("COMPILED_CODEMODE startup passed");
  expect(stdout).toContain("COMPILED_CODEMODE reload passed");
  expect(stdout).toMatch(/COMPILED_CODEMODE kernel file:\/\/\/\$bunfs\/root\/src\/extensions\/codemode\/kernel-worker\.js/);
}, 60_000);
