// Early T-570 checkpoint: actual host prompt/tool profiles, deterministic transport only.
// This does not evaluate workflow quality or the completed standard preset.
import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";

const early = ["context_files", "skills", "prompt_templates", "compaction", "session_export"];

test("early strict/standard static profiles retain raw prompts and schemas for each shipped role", async () => {
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-usage-baseline-"));
  const originalFetch = globalThis.fetch;
  try {
    for (const tree of ["src/roles", "src/extensions", "assets"]) {
      cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
    }
    const cwd = join(work, "project");
    const home = join(work, "home");
    mkdirSync(cwd);
    mkdirSync(join(home, ".agents/skills/baseline"), { recursive: true });
    writeFileSync(join(home, ".agents/skills/baseline/SKILL.md"),
      "---\nname: baseline\ndescription: Fixed checkpoint skill\n---\nCheck evidence before reporting success.\n");
    writeFileSync(join(cwd, "AGENTS.md"), "Use targeted tests. Do not change unrelated files.\n");
    restrictEnvironment(process.env);
    for (const role of ["worker", "reviewer", "pm"] as const) {
      for (const preset of ["strict", "standard"] as const) {
        const dir = join(work, `${role}-${preset}`);
        mkdirSync(dir);
        const modules = await loadModules(assets.dir, role);
        const transport = modules.auth.transport?.(authContext());
        if (!transport) throw new Error("fixture transport missing");
        const network = guardedFetch(transport);
        globalThis.fetch = network.fetch;
        const sdk = await import("@earendil-works/pi-coding-agent");
        const enabled = preset === "strict" ? [] : [...early,
          ...(role === "worker" ? ["todo", "background_shell", "codemode"] :
            role === "reviewer" ? ["codemode"] : ["todo"])];
        const identity = { store: "st-baseline", project: "baseline", role, generation: 1 };
        const host = await HostSession.create({
          sdk, identity, cwd, root: cwd, home, agentDir: join(dir, "agent"), model: "gpt-6.1-sol",
          capabilities: { catalog: 4, digest: sha256Hex(JSON.stringify(enabled)), preset, enabled },
          runtimeDir: join(dir, "runtime"),
          guidance: { projectText: true, settings: { skills: { user_dirs: ["~/.agents/skills"] },
            background_shell: { max_jobs: 2, max_output_bytes: 4096, max_runtime_secs: 60 } } },
          modules, network, events: new EventLog(1),
          client: new CorralClient({ corral: "/unused", root: cwd, identity, hostCap: "h".repeat(64), sessionId: () => "" }),
          sessionManager: sdk.SessionManager.inMemory(cwd),
        });
        try {
          await host.session.bindExtensions({});
          // Normalize only the disposable fixture prefix; retain the complete text/schema.
          const prompt = host.session.systemPrompt.split(work).join("<fixture>")
            .split(assets.dir).join("<assets>").split(PACKAGE_ROOT).join("<package>");
          const tools = host.session.agent.state.tools.map(({ name, description, parameters }) => ({ name, description, parameters }));
          const toolJson = JSON.stringify(tools);
          const bytes = (text: string) => Buffer.byteLength(text);
          console.log(`EARLY_STATIC ${JSON.stringify({
            role, preset, estimator: "utf8-json-bytes/4-v1", enabled,
            model: "gpt-6.1-sol", inference: false, prompt, tools,
            prompt_bytes: bytes(prompt), tool_bytes: bytes(toolJson),
            prompt_tokens_estimate: Math.ceil(bytes(prompt) / 4),
            tool_tokens_estimate: Math.ceil(bytes(toolJson) / 4),
            prompt_sha256: sha256Hex(prompt), tools_sha256: sha256Hex(toolJson),
          })}`);
          expect(tools.length).toBeGreaterThan(0);
          expect(host.loadout.includes("codemode")).toBe(preset === "standard" && role !== "pm");
          expect(host.loadout.includes("corral_todo")).toBe(preset === "standard" && role !== "reviewer");
        } finally { await host.close(); }
      }
    }
    for (const role of ["planner", "plan_reviewer"]) {
      console.log(`EARLY_STATIC ${JSON.stringify({ role, runnable: false,
        reason: "native role module unshipped; strict disabled-role trial is not a zero-cost success",
        prompt_tokens_estimate: null, tool_tokens_estimate: null })}`);
    }
  } finally {
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 60_000);
