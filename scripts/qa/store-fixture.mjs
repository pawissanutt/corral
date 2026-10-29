// Exact synthetic writes only. Ordinary QA readbacks must still use the candidate CLI.
import assert from "node:assert/strict";
import { mkdtemp, readFile, realpath, writeFile } from "node:fs/promises";
import { rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const source = fileURLToPath(new URL("./store-fixture.rs", import.meta.url));
const repo = resolve(dirname(source), "../..");
let executable;

async function command(argv, options = {}) {
  const child = Bun.spawn(argv, { cwd: repo, stdout: "pipe", stderr: "pipe",
    stdin: "ignore", ...options });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  assert.equal(code, 0, `${argv.join(" ")}\n${stderr}`);
  return stdout;
}

async function build() {
  // Cargo identifies the exact rlib (and its dependencies), even with several builds in target.
  const output = await command(["cargo", "build", "--locked", "-p", "corral-core", "--message-format=json"]);
  const artifacts = output.split("\n").filter(Boolean).map(JSON.parse);
  const artifact = artifacts.findLast(message =>
    message.reason === "compiler-artifact" && message.target.name === "corral_core");
  const library = artifact?.filenames.find(path => path.endsWith(".rlib"));
  assert(library, "cargo must report the corral-core rlib");
  const anyhow = artifacts.findLast(message =>
    message.reason === "compiler-artifact" && message.target.name === "anyhow")
    ?.filenames.find(path => path.endsWith(".rlib"));
  assert(anyhow, "cargo must report the anyhow rlib");
  const directory = await mkdtemp(join(tmpdir(), "corral-qa-writer-"));
  process.once("exit", () => rmSync(directory, { recursive: true, force: true }));
  const binary = join(directory, "store-fixture");
  await command(["rustc", "--edition=2024", source, "-L", `dependency=${join(dirname(library), "deps")}`,
    "--extern", `corral_core=${library}`, "--extern", `anyhow=${anyhow}`, "-o", binary]);
  return binary;
}

export async function fixtureWriter(root, env = process.env) {
  const canonical = await realpath(root);
  assert(canonical.startsWith(`${await realpath(tmpdir())}/`) ||
    canonical.startsWith(`${repo}/.omo/evidence/`), "Only disposable QA roots may be marked");
  const marker = join(canonical, ".corral-qa-fixture");
  try { await writeFile(marker, "synthetic-only\n", { flag: "wx" }); }
  catch (error) {
    if (error.code !== "EEXIST") throw error;
    assert.equal(await readFile(marker, "utf8"), "synthetic-only\n", "Invalid fixture marker");
  }
  executable ??= build();
  const binary = await executable;
  const apply = (operation, input) => command([binary, canonical, operation], {
    stdin: new TextEncoder().encode(input), env,
  });
  return {
    putTask: (task, body = task.body ?? "") =>
      apply("put-task", `---\n${JSON.stringify(task, null, 2)}\n---\n${body}`),
    appendEvents: events => apply("append-events", events),
    replaceEvents: events => apply("replace-events", events),
    removeTask: id => apply("remove-task", id),
    corruptTask: id => apply("corrupt-task", id),
    tourSeen: () => apply("tour-seen", ""),
    tourReset: () => apply("tour-reset", ""),
    reviewedArtifacts: paths => apply("reviewed-artifacts", paths.join("\n")),
    cacheRoot: async () => (await apply("cache-root", "")).trim(),
  };
}
