import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { bindBuildResume, parseBuildFlags } from "../src/build/launch.ts";
import { loadSession } from "../src/build/session-store.ts";
import { prepareCodeCheckout } from "../src/build/worktree.ts";
import { connect } from "./support/bridge-client.ts";
import { scriptedFixtureInstall } from "./support/fixture-install.ts";
import { launch } from "./support/pi-process.ts";
import { within } from "./support/wait.ts";

test("Code preserves invocation cwd in an unborn Git checkout", () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "corral-code-git-")));
  try {
    const cwd = join(root, "nested");
    mkdirSync(join(root, ".git", "objects"), { recursive: true });
    mkdirSync(join(root, ".git", "refs", "heads"), { recursive: true });
    writeFileSync(join(root, ".git", "HEAD"), "ref: refs/heads/main\n");
    mkdirSync(cwd);
    const code = parseBuildFlags([], "/assets", {}, cwd, "code");
    expect(code.launch).toMatchObject({ repoRoot: root, here: true, trustedCode: true });
    mkdirSync(join(root, ".corral-next"));
    writeFileSync(join(root, ".corral-next", "pi.toml"), '[startup]\nmodel = "openai-codex/gpt-6.2-sol"\n');
    const managed = parseBuildFlags(["--headless", "--id", "B-code"], "/assets",
      { CORRAL_PI_MANAGED_BUILD: "1" }, cwd, "code", root);
    expect(managed.launch).toMatchObject({ provider: "openai-codex", model: "openai-codex/gpt-6.2-sol", trustedCode: true, here: true });
    expect(managed.local.codeCwd).toBe(cwd);
    expect(code.local.codeCwd).toBe(cwd);
    // A terminal may have resumed into a different workspace (or an unbound Code session).
    const ambient = { CORRAL_NEXT_ROOT: join(root, "original-workspace") };
    const next = (workspace: string | null) => parseBuildFlags([], "/assets", ambient, cwd, "code", workspace).launch;
    expect(next(root).workspaceRoot).toBe(root);
    expect(next(null).workspaceRoot).toBeUndefined();
    expect(prepareCodeCheckout(code.launch.repoRoot, code.local.codeCwd!)).toEqual({
      worktree: cwd, base: null, branch: "main", baseBranch: "main", repository: true,
    });
    expect(parseBuildFlags(["--here"], "/assets", {}, cwd).launch.trustedCode).toBeUndefined();
    expect(existsSync(join(root, ".worktrees"))).toBeFalse();
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("Code quits normally outside Git and resumes its empty transcript explicitly", async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "corral-code-")));
  const data = join(root, "data"), cwd = join(root, "local"), other = join(root, "elsewhere");
  const id = "B-code1", model = "openai-codex/gpt-6.2-sol";
  try {
    mkdirSync(cwd);
    mkdirSync(other);
    const assets = scriptedFixtureInstall(root, [{ text: "ready" }]);
    const env = { PATH: process.env.PATH, HOME: root, TMPDIR: root };
    const run = async (args: string[], launchCwd: string, socket: string) => {
      const child = launch(assets, ["code", "--headless", "--data-dir", data, "--socket", socket, ...args], launchCwd, env);
      try {
        await child.output(/session socket ready/);
        const record = loadSession(data, id);
        expect(record).toMatchObject({ worktree: cwd, repoRoot: cwd, here: true, repository: false, bare: true, trustedCode: true });
        expect(existsSync(record.sessionFile!)).toBeTrue();
        const connection = await connect(socket);
        try {
          expect((await connection.request({ version: 2, id: "end", command: "shutdown", args: {} })).ok).toBeTrue();
        } finally { connection.close(); }
        expect(await within(child.exit, 10_000, "Code shutdown")).toBe(0);
        expect(await child.output(/Resume it with/)).toBe(`Resume it with \`corral-pi code --resume ${id} --data-dir '${data}'\`.`);
        return record.sessionFile;
      } finally { await child.stop(); }
    };
    const transcript = await run(["--id", id, "--model", model], cwd, join(root, "start.sock"));
    const ordinaryResume = parseBuildFlags(["--resume", id], assets, env, other).launch;
    const record = loadSession(data, id);
    expect(() => bindBuildResume(ordinaryResume, record)).toThrow(`code --resume ${id}`);
    const wrong = launch(assets, ["build", "--data-dir", data, "--resume", id], other, env);
    try {
      expect(await within(wrong.exit, 10_000, "Build refuses Code resume")).toBe(2);
      expect(wrong.stderr()).toContain(`corral-pi code --resume ${id}`);
    } finally { await wrong.stop(); }
    const trustedResume = parseBuildFlags(["--resume", id], assets, env, other, "code").launch;
    expect(() => bindBuildResume(trustedResume, { ...record, trustedCode: undefined })).toThrow(`build --resume ${id}`);
    expect(await run(["--resume", id], other, join(root, "resume.sock"))).toBe(transcript);
  } finally { rmSync(root, { recursive: true, force: true }); }
}, 30_000);
