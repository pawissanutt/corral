import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseBuildFlags } from "../build/launch.ts";
import { parseRunFlags } from "./config.ts";


test("flags name the engine run or task reviewer without a legacy launch message", () => {
  const flags = ["--dir", "runs/T-1/R1", "--socket", "socket", "--prompt-file", "prompt.md",
    "--corral", "/test/corral-next", "--model", "anthropic/claude-opus-4-8", "--effort", "high"];
  const env = { CORRAL_NEXT_ROOT: "/fixture", CORRAL_TASK: "T-1", CORRAL_RUN: "R1" };
  const run = parseRunFlags("run", flags, "/assets", { ...env, CORRAL_ACTOR: "agent" }, "/fixture/work");
  expect(run.identity).toMatchObject({ role: "worker", task: "T-1", run: "R1" });
  expect(run.local).toMatchObject({ id: "T-1/R1", dir: "/fixture/work/runs/T-1/R1", socket: "/fixture/work/socket" });
  expect(run).toMatchObject({ root: "/fixture", corral: "/test/corral-next", provider: "anthropic", effort: "high",
    promptFile: "/fixture/work/prompt.md" });
  expect(parseRunFlags("run", flags, "/assets", { ...env, CORRAL_ACTOR: "reviewer" }, "/fixture/work").local?.id)
    .toBe("T-1/reviewer");
});

test("terminal chat discovers the nearest workspace and keeps the launch directory", () => {
  const outer = mkdtempSync(join(tmpdir(), "corral-terminal-chat-"));
  try {
    const root = join(outer, "workspace"), cwd = join(root, "project", "nested");
    for (const workspace of [outer, root]) {
      mkdirSync(join(workspace, ".corral-next"), { recursive: true });
      writeFileSync(join(workspace, ".corral-next", "corral.toml"), "");
    }
    mkdirSync(cwd, { recursive: true });
    const dir = join(root, ".corral-next", "terminal-chat");
    expect(parseRunFlags("chat", [], "/assets", {}, cwd)).toMatchObject({
      root, cwd, corral: "corral-next", local: { id: "terminal-chat", dir },
      sessionDir: join(dir, "sessions"), freshSession: false,
    });
    expect(parseRunFlags("chat", ["--new"], "/assets", {}, cwd)).toMatchObject({ local: { dir }, freshSession: true });
    expect(parseRunFlags("chat", [], "/assets", { CORRAL_NEXT_ROOT: outer }, cwd).root).toBe(outer);
  } finally { rmSync(outer, { recursive: true, force: true }); }
});

test("only implicit terminal chat requires a workspace", () => {
  const cwd = mkdtempSync(join(tmpdir(), "corral-chat-no-workspace-"));
  try {
    expect(() => parseRunFlags("chat", [], "/assets", {}, cwd)).toThrow();
    const dir = join(cwd, "chat", "thread");
    expect(parseRunFlags("chat", ["--dir", "chat/thread", "--corral", "bin/corral-next", "--socket", "socket"],
      "/assets", {}, cwd)).toMatchObject({
      root: cwd, cwd, corral: join(cwd, "bin", "corral-next"), freshSession: false,
      local: { id: "thread", dir, socket: join(cwd, "socket") }, sessionDir: join(dir, "sessions"),
    });
  } finally { rmSync(cwd, { recursive: true, force: true }); }
});

test("fallback uses explicit ordered provider models and cannot point at the launch model", () => {
  const parse = (flags: readonly string[]) => parseRunFlags("chat", ["--dir", "test", "--model", "anthropic-api/claude-opus-5-5", ...flags],
    "/assets", { CORRAL_NEXT_ROOT: "/fixture" }, "/fixture");
  expect(parse(["--fallback", "anthropic/claude-opus-5-5", "--fallback", "openai/gpt-5.4"])).toMatchObject({
    provider: "anthropic-api", fallback: [{ provider: "anthropic", model: "claude-opus-5-5" }, { provider: "openai", model: "gpt-5.4" }],
  });
  for (const invalid of ["unknown/model", "gpt-5.4", "openai/", "anthropic-api/claude-opus-5-5"]) {
    expect(() => parse(["--fallback", invalid])).toThrow();
  }
  expect(() => parse(["--connection", "anthropic-api"])).toThrow("--model anthropic-api/<model>");
  expect(() => parse(["--paid-route", "openai/gpt-5.4"])).toThrow("--fallback");
  expect(() => parse(["--max-usd", "3"])).toThrow();
});

test("startup model input stays separate from explicit choices and managed launch defaults", () => {
  const root = mkdtempSync(join(tmpdir(), "corral-startup-model-"));
  try {
    const settings = join(root, ".corral-next");
    mkdirSync(settings);
    writeFileSync(join(settings, "corral.toml"), '[profiles.demo]\nharness = "pi"\nmodel = "anthropic/claude-opus-4-8"\n');
    const piToml = join(settings, "pi.toml");
    const chat = (...flags: string[]) => parseRunFlags("chat", flags, "/assets", {}, root);
    const build = (...flags: string[]) => parseBuildFlags(flags, "/assets", {}, root);
    const daemonBuild = (...flags: string[]) => parseBuildFlags(["--headless", "--id", "B-managed", ...flags],
      "/assets", { CORRAL_PI_MANAGED_BUILD: "1" }, root).launch;
    expect(chat().startupModel).toBeNull();
    expect(build().startupModel).toBeNull();
    expect(build("--headless", "--id", "B-managed").launch.model).toBeUndefined();
    expect(daemonBuild().model).toBe("gpt-6.1-sol");
    writeFileSync(piToml, '[startup]\nmodel = "openai-codex/gpt-6.2-sol"\n');
    expect(chat().startupModel).toBe("openai-codex/gpt-6.2-sol");
    expect(build().startupModel).toBe("openai-codex/gpt-6.2-sol");
    expect(daemonBuild().model).toBe("openai-codex/gpt-6.2-sol");
    expect(daemonBuild("--profile", "demo").model).toBe("anthropic/claude-opus-4-8");
    for (const launch of [chat, (...flags: string[]) => build(...flags).launch]) {
      expect(launch().model).toBeUndefined();
      expect(launch("--model", "anthropic/claude-opus-4-8").provider).toBe("anthropic");
      expect(launch("--provider", "anthropic")).toMatchObject({ provider: "anthropic", model: undefined });
    }
    expect(build("--profile", "demo").launch.model).toBe("anthropic/claude-opus-4-8");
    expect(build("--resume", "B-existing").startupModel).toBeNull();
    expect(() => build("--resume=")).toThrow("Invalid build session id");
    expect(parseRunFlags("chat", ["--dir", "managed"], "/assets", {}, root).model).toBe("gpt-6.1-sol");
    expect(parseRunFlags("run", ["--dir", "managed"], "/assets", {}, root).model).toBe("gpt-6.1-sol");
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("Build launch and child profiles use provider-qualified models and preserve fallback order", () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-profile-fallback-"));
  try {
    const settings = join(dir, ".corral-next");
    mkdirSync(settings);
    const path = join(settings, "corral.toml");
    const profile = '[profiles.demo]\nharness = "pi"\nmodel = "anthropic-api/claude-opus-5-5"\n';
    writeFileSync(path, `${profile}fallback = ["openai-codex/gpt-6.2-sol", "anthropic/claude-opus-5-5"]\n`);
    writeFileSync(join(settings, "pi.toml"), '[build.profiles.metered]\nmodel = "anthropic-api/claude-opus-5-5"\n');
    const parse = () => parseBuildFlags(["--here", "--data-dir", dir, "--profile", "demo"], "/assets", { CORRAL_NEXT_ROOT: dir }, dir).launch;
    const launch = parse();
    expect(launch).toMatchObject({ provider: "anthropic-api", model: "anthropic-api/claude-opus-5-5",
      fallback: [{ provider: "openai-codex", model: "gpt-6.2-sol" }, { provider: "anthropic", model: "claude-opus-5-5" }] });
    expect(launch.settings.profiles).toMatchObject([{ name: "metered", model: "anthropic-api/claude-opus-5-5" }]);
    writeFileSync(path, `${profile}args = ["--connection", "anthropic-api"]\n`);
    expect(parse).toThrow("--model anthropic-api/<model>");
    writeFileSync(path, profile);
    writeFileSync(join(settings, "pi.toml"), '[build.profiles.metered]\nmodel = "anthropic/claude-opus-5-5"\nconnection = "anthropic-api"\n');
    expect(parse).toThrow('model = "anthropic-api/<model>"');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
