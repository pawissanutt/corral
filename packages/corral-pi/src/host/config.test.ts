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
