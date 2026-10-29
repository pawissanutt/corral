// Handler prototype identity and pi-ai's lazily imported Bedrock client both depend on bundling, so
// the fake Bedrock turn also runs as an executable built with scripts/build-pi-host.sh's flags.
import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { CONVERSE_URL, FIXTURE_KEY } from "./fixtures/bedrock/fake-aws.ts";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

test("a compiled executable sends the same single signed POST, decodes the reply and refuses raw node:https", () => {
  const script = readFileSync(join(PACKAGE_ROOT, "../../scripts/build-pi-host.sh"), "utf8");
  const flags = /bun build ((?:--[a-z-]+ )+)\\\n/.exec(script)?.[1]?.trim().split(" ");
  expect(flags).toEqual(["--compile", "--no-compile-autoload-dotenv", "--no-compile-autoload-bunfig"]);
  const output = join(mkdtempSync(join(tmpdir(), "corral-bedrock-compiled-")), "bedrock-entry");
  const entry = fileURLToPath(new URL("fixtures/bedrock/compiled-entry.ts", import.meta.url));
  const build = spawnSync(process.execPath, ["build", ...flags!, entry, "--outfile", output], {
    cwd: PACKAGE_ROOT, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 240_000,
  });
  expect([build.status, build.stderr]).toEqual([0, ""]);
  const run = spawnSync(output, [], {
    encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 60_000,
    env: { PATH: process.env.PATH, TMPDIR: tmpdir() },
  });
  expect([run.status, run.stderr]).toEqual([0, ""]);
  expect(JSON.parse(run.stdout)).toEqual({
    stopReason: "stop", errorMessage: null, content: [{ type: "text", text: "OK" }], usage: [11, 2, 13],
    requests: [["POST", CONVERSE_URL, FIXTURE_KEY]],
    ledger: { allowed: 1, refused: 1, refusedDestinations: ["https://bedrock-runtime.us-west-2.amazonaws.invalid/"] },
    raw: "network_refused",
  });
}, 300_000);
