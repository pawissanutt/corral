import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseBuildFlags } from "../build/launch.ts";
import { parseRunFlags } from "./config.ts";
import { runHost } from "./main.ts";
import { resolveStartupModel, StartupModelRequired } from "./startup-model.ts";

test("headless startup requires a choice before creating a session and preserves explicit models", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-headless-startup-"));
  try {
    const env = { CORRAL_NEXT_ROOT: root };
    const chat = parseRunFlags("chat", ["--headless"], "/assets", env, root);
    await expect(runHost(chat)).rejects.toThrow(StartupModelRequired);
    expect(existsSync(chat.local.dir)).toBeFalse();

    const settings = join(root, ".corral-next");
    // No workspace config directory was made by the refused host.
    expect(existsSync(settings)).toBeFalse();
    mkdirSync(settings);
    writeFileSync(join(settings, "pi.toml"), '[startup]\nmodel = "llama.cpp/not-available-in-build"\n');
    const flags = ["--headless", "--data-dir", "data"];
    const refused = spawnSync(process.execPath, ["--no-env-file", fileURLToPath(new URL("../cli.ts", import.meta.url)), "build", ...flags],
      { cwd: root, encoding: "utf8", env: { ...env, HOME: root, PATH: process.env.PATH } });
    expect(refused.status).toBe(2);
    expect(refused.stderr).toContain("Pass --model <provider/model>.");
    const { launch, startupModel: model } = parseBuildFlags(flags, "/assets", env, root);
    const choose = (flags: typeof launch) => resolveStartupModel({ ...flags,
      workspace: { root, corral: "/unused", model }, role: "build", headless: true });
    await expect(choose({ ...launch, provider: "anthropic" })).rejects.toThrow(StartupModelRequired);
    expect(await choose({ ...launch, provider: "anthropic", model: "anthropic/explicit" })).toEqual({
      provider: "anthropic", model: "anthropic/explicit",
    });
    expect(existsSync(launch.dataRoot)).toBeFalse();
    expect(existsSync(join(root, ".worktrees"))).toBeFalse();
  } finally { rmSync(root, { recursive: true, force: true }); }
});
