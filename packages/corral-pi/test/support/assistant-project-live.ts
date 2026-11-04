// Run only inside scripts/test-herdr.sh's private, plugin-free server.
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import { buildFixtureInstall, PACKAGE_ROOT } from "./fixture-install.ts";
import { ok, privateHerdr, privateRoot, REPO } from "./workspace.ts";

async function main(): Promise<void> {
  const parent = privateRoot();
  if (!parent || !privateHerdr()) throw new Error("requires scripts/test-herdr.sh private server");
  const root = mkdtempSync(join(parent, "assistant-project-"));
  const app = join(root, "app");
  mkdirSync(app);
  const assets = buildFixtureInstall({ parent: root, omit: ["role"] });
  let workspace: string | undefined;
  try {
    cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
    ok(["git", "-C", app, "init", "-q", "-b", "main"]);
    writeFileSync(join(app, "README.md"), "fixture\n");
    writeFileSync(join(app, ".gitignore"), ".worktrees/\n");
    ok(["git", "-C", app, "add", "."]);
    ok(["git", "-C", app, "commit", "-qm", "fixture"]);
    const created = z.object({ result: z.object({
      workspace: z.object({ workspace_id: z.string() }),
      root_pane: z.object({ pane_id: z.string() }),
    }) }).parse(JSON.parse(ok(["herdr", "--session", "corral-test", "workspace", "create",
      "--cwd", app, "--label", "assistant-project-fixture", "--no-focus"])));
    workspace = created.result.workspace.workspace_id;
    const target = resolve(REPO, process.env.CARGO_TARGET_DIR ?? "target");
    const child = spawn("cargo", ["test", "-p", "corral-ops", "--lib",
      "agent::pi::assistant_tests::assistant_project_private_host_relink", "--", "--ignored", "--exact", "--nocapture"], {
      cwd: REPO, stdio: ["ignore", "inherit", "inherit"],
      env: { ...process.env, CORRAL_ASSISTANT_PROJECT_LIVE: JSON.stringify({
        root, assets: assets.dir, pane: created.result.root_pane.pane_id,
        corral: join(target, "debug/corral"),
        driver: fileURLToPath(new URL("./assistant-project-host.ts", import.meta.url)),
      }) },
    });
    const status = await new Promise<number | null>((resolve, reject) => {
      child.once("error", reject);
      child.once("exit", resolve);
    });
    if (status !== 0) throw new Error(`private assistant relink scenario exited ${status}`);
  } finally {
    if (workspace) ok(["herdr", "--session", "corral-test", "workspace", "close", workspace]);
    assets.cleanup();
    rmSync(root, { recursive: true, force: true });
  }
}

main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exitCode = 1;
});
