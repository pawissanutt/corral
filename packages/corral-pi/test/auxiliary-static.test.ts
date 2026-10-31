// Additive actual-host prompt/schema profiles, not an inference comparison.
import { expect, test } from "bun:test";
import { cpSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import type { Role } from "../src/roles/types.ts";

test("auxiliary static deltas record actual host prompts and tool schemas for all roles", async () => {
  const assets = buildFixtureInstall();
  const cwd = mkdtempSync(join(tmpdir(), "corral-aux-static-"));
  const originalFetch = globalThis.fetch;
  try {
    for (const tree of ["src/roles", "assets"]) cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
    for (const role of ["worker", "reviewer", "pm", "planner", "plan_reviewer"] as Role[]) {
      const modules = await loadModules(assets.dir, role);
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("fixture transport missing");
      const network = guardedFetch(transport);
      globalThis.fetch = network.fetch;
      const identity = { store: "st-demo", project: "demo", role, generation: 1, task: "T-571", run: "R1" };
      async function profile(enabled: string[]) {
        const host = await HostSession.create({ sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol", modules,
          network, events: new EventLog(1), capabilities: { catalog: 6, digest: "a".repeat(64), preset: "full", enabled },
          guidance: { projectText: false, settings: { subagents: { max_parallel: 3, model: null, max_turns: 12, timeout_secs: 120 }, oracle: { model: null, timeout_secs: 120 } } },
          client: new CorralClient({ corral: "/unused", root: cwd, identity, hostCap: "h".repeat(64), sessionId: () => "" }),
          sessionManager: sdk.SessionManager.inMemory(cwd),
        });
        try {
          await host.session.bindExtensions({});
          const prompt = host.session.systemPrompt.split(cwd).join("<fixture>").split(assets.dir).join("<assets>");
          const tools = host.session.agent.state.tools.map(({ name, description, parameters }) => ({ name, description, parameters }));
          return { prompt, tools, promptBytes: Buffer.byteLength(prompt), toolBytes: Buffer.byteLength(JSON.stringify(tools)) };
        } finally { await host.close(); }
      }
      const base = await profile([]);
      for (const preset of ["strict", "standard", "full"]) {
        const explore = role !== "pm" && (preset === "full" || (preset === "standard" && role !== "reviewer"));
        const enabled = [...(explore ? ["subagents"] : []), ...(preset === "full" ? ["oracle"] : [])];
        const candidate = enabled.length ? await profile(enabled) : base;
        const promptDelta = candidate.promptBytes - base.promptBytes;
        const toolDelta = candidate.toolBytes - base.toolBytes;
        console.log(`AUXILIARY_STATIC ${JSON.stringify({ role, preset, estimator: "utf8-json-bytes/4-v1", inference: false,
          scope: "additive auxiliary delta over same role with auxiliary disabled; not completed standard comparison", enabled,
          prompt_bytes_delta: promptDelta, tool_bytes_delta: toolDelta,
          prompt_tokens_delta_estimate: Math.ceil(candidate.promptBytes / 4) - Math.ceil(base.promptBytes / 4),
          tool_tokens_delta_estimate: Math.ceil(candidate.toolBytes / 4) - Math.ceil(base.toolBytes / 4),
          baseline: base, candidate })}`);
        if (!enabled.length) { expect(promptDelta).toBe(0); expect(toolDelta).toBe(0); }
        else { expect(promptDelta).toBeGreaterThanOrEqual(0); expect(toolDelta).toBeGreaterThan(0); }
      }
    }
  } finally { globalThis.fetch = originalFetch; assets.cleanup(); rmSync(cwd, { recursive: true, force: true }); }
}, 30_000);
