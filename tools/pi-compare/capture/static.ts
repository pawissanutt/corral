// Offline research capture. Real Rust defaults + real host; fake auth, private text and MCP.
// No inference, corral operations, external web requests or language-server launches.
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import type { CapabilityGuidance } from "../../../packages/corral-pi/src/capabilities/guidance.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "../../../packages/corral-pi/test/support/fixture-install.ts";
import { mcpServer } from "../../../packages/corral-pi/test/support/mcp-server.ts";
import { loadModules } from "../../../packages/corral-pi/src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../../packages/corral-pi/src/host/restrict.ts";
import { HostSession, authContext } from "../../../packages/corral-pi/src/host/session.ts";
import { EventLog } from "../../../packages/corral-pi/src/bridge/events.ts";
import { CorralClient } from "../../../packages/corral-pi/src/tools/corral.ts";
import { mcpToolName } from "../../../packages/corral-pi/src/capabilities/mcp.ts";
import { fixtureTransport } from "../../../packages/corral-pi/test/fixtures/auth/index.ts";
import { nextEvent } from "../../../packages/corral-pi/test/support/wait.ts";
import { writeFakeCorral } from "../../../packages/corral-pi/test/support/fake-corral.ts";

const hash = (s: string | Buffer) => createHash("sha256").update(s).digest("hex");
const [binary, output] = process.argv.slice(2);
if (!binary || !output) throw new Error("usage: bun capture/static.ts RESOLVER NEW_OUTPUT_DIR");
const out = resolve(output);
mkdirSync(out); // Never overwrite evidence.
const save = (name: string, value: unknown) => writeFileSync(join(out, name), JSON.stringify(value, null, 2) + "\n", { flag: "wx" });
const work = mkdtempSync(join(tmpdir(), "corral-final-profile-"));
const assets = buildFixtureInstall();
const peer = await mcpServer();
const originalFetch = globalThis.fetch;
const compat = JSON.parse(readFileSync(join(PACKAGE_ROOT, "compat.json"), "utf8"));
const git = (args: string[]) => {
  const result = spawnSync("git", args, { stdio: ["ignore", "pipe", "pipe"] });
  if (result.status !== 0) throw new Error(result.stderr.toString());
  return result.stdout.toString().trim();
};
const sourceFiles = git(["ls-files", "packages/corral-pi", "crates/corral-core/src/agent", "crates/corral-core/src/config/agent_capabilities.rs"])
  .split("\n").map(path => ({ path, sha256: hash(readFileSync(path)) }));
save("controls.json", { version: 1, source_commit: git(["rev-parse", "HEAD"]), source_files: sourceFiles,
  resolver_sha256: hash(readFileSync(binary)), capture_script_sha256: hash(readFileSync(import.meta.filename)),
  estimator: "utf8-json-bytes/4-v1", model: "gpt-6.1-sol",
  inference: false, supported: compat.capabilities, project: "baseline", project_text: "trusted",
  normalization: ["temporary work prefix", "temporary assets prefix", "package root", "ephemeral MCP origin"],
  resources: { inert: "empty home directories, no MCP servers, no SearXNG; public web default",
    configured: "one fixed home skill/template, read MCP peer, SearXNG fixture origin and lazy LSP command" },
  workflow_comparison: false });
try {
  for (const tree of ["src/roles", "src/extensions", "assets"]) {
    cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
  }
  restrictEnvironment(process.env);
  const sdk = await import("../../../packages/corral-pi/node_modules/@earendil-works/pi-coding-agent");
  for (const resource of ["inert", "configured"] as const) {
    const cwd = join(work, resource, "project"), home = join(work, resource, "home");
    mkdirSync(cwd, { recursive: true });
    mkdirSync(join(home, ".agents/skills"), { recursive: true });
    mkdirSync(join(home, ".agents/prompts"), { recursive: true });
    writeFileSync(join(cwd, "AGENTS.md"), "Use targeted tests. Do not change unrelated files.\n");
    if (resource === "configured") {
      mkdirSync(join(home, ".agents/skills/baseline"));
      writeFileSync(join(home, ".agents/skills/baseline/SKILL.md"),
        "---\nname: baseline\ndescription: Fixed final profile skill\n---\nCheck evidence before reporting success.\n");
      writeFileSync(join(home, ".agents/prompts/evidence.md"), "Summarize observed evidence, not assumed outcomes.\n");
    }
    for (const preset of ["strict", "standard"] as const) {
      const config: Record<string, unknown> = { preset };
      if (resource === "configured") for (const role of ["worker", "reviewer", "pm", "planner", "plan_reviewer"]) {
        config[role] = { mcp: { servers: { docs: { projects: ["baseline"], url: peer.url,
          tools: { lookup: "read" }, resources: ["fixture://public/*"] } } },
          ...(role !== "pm" ? { web: { searxng_url: "http://127.0.0.1:8888/search" } } : {}),
          ...(["worker", "planner", "plan_reviewer"].includes(role) ? { lsp: { languages: {
            fixture: { command: "false", args: [], extensions: { ".profile": "fixture" }, root_markers: ["AGENTS.md"] },
          } } } : {}) };
      }
      const request = { config, supported: compat.capabilities };
      const resolved = spawnSync(resolve(binary), [], { input: JSON.stringify(request) });
      if (resolved.status !== 0) throw new Error(resolved.stderr.toString());
      const snapshots = JSON.parse(resolved.stdout.toString());
      save(`${resource}-${preset}-resolution.json`, { request, snapshots });
      for (const snapshot of snapshots) {
        const { role } = snapshot;
        const key = `${resource}-${role}-${preset}`;
        if (["planner", "plan_reviewer"].includes(role) && !snapshot.features.role_enabled.enabled) {
          save(`${key}.json`, { role, preset, resource, runnable: false, snapshot,
            reason: "strict disables native role; not a zero-cost success", prompt_tokens_estimate: null, tool_tokens_estimate: null });
          continue;
        }
        const enabled = Object.entries(snapshot.features).filter(([name, state]) =>
          name !== "lessons" && (state as { enabled: boolean }).enabled).map(([name]) => name);
        const settings = Object.fromEntries(Object.entries(snapshot.features).flatMap(([name, state]) =>
          (state as { settings?: unknown }).settings ? [[name, (state as { settings: unknown }).settings]] : [])) as CapabilityGuidance["settings"];
        const capabilities = { catalog: snapshot.catalog, preset, digest: snapshot.digest, enabled,
          ...(snapshot.web ? { web: snapshot.web } : {}), ...(snapshot.mcp ? { mcp: snapshot.mcp } : {}) };
        const modules = await loadModules(assets.dir, role);
        const transport = modules.auth.transport?.(authContext());
        if (!transport) throw new Error("fixture provider missing");
        const network = guardedFetch(transport, undefined, "openai-codex", undefined,
          snapshot.web ? { settings: snapshot.web, send: async () => { throw new Error("static web never executes"); } } : undefined,
          snapshot.mcp ? { origins: resource === "configured" ? [new URL(peer.url).origin] : [], send: originalFetch.bind(globalThis) } : undefined);
        globalThis.fetch = network.fetch as typeof globalThis.fetch;
        const dir = join(work, key);
        mkdirSync(dir);
        const identity = { store: "st-baseline", project: "baseline", role, task: "T-579", run: "R1", generation: 1 };
        const events = new EventLog(1);
        const host = await HostSession.create({ sdk, identity, cwd, root: cwd, home, agentDir: join(dir, "agent"),
          model: "gpt-6.1-sol", capabilities, guidance: { projectText: snapshot.project_text, settings },
          runtimeDir: join(dir, "runtime"), modules, network, events,
          client: new CorralClient({ corral: writeFakeCorral(dir).path, root: cwd, identity, hostCap: "h".repeat(64), sessionId: () => "" }),
          sessionManager: sdk.SessionManager.inMemory(cwd) });
        try {
          await host.session.bindExtensions({});
          const normalize = (s: string) => s.split(cwd).join("<project>").split(home).join("<home>")
            .split(dir).join("<arm>").split(work).join("<fixture>").split(assets.dir).join("<assets>")
            .split(PACKAGE_ROOT).join("<package>").split(new URL(peer.url).origin).join("http://<mcp-fixture>");
          const capture = (stage: string) => {
            const prompt = normalize(host.session.systemPrompt);
            const tools = JSON.parse(normalize(JSON.stringify(host.session.agent.state.tools.map(({ name, description, parameters }) => ({ name, description, parameters })))));
            const toolJson = JSON.stringify(tools), bytes = (s: string) => Buffer.byteLength(s);
            save(`${key}${stage === "startup" ? "" : "-activated"}.json`, { role, preset, resource, stage, runnable: true,
              snapshot, model: "gpt-6.1-sol", inference: false, estimator: "utf8-json-bytes/4-v1", enabled, prompt, tools,
              prompt_bytes: bytes(prompt), tool_bytes: bytes(toolJson), prompt_tokens_estimate: Math.ceil(bytes(prompt) / 4),
              tool_tokens_estimate: Math.ceil(bytes(toolJson) / 4), prompt_sha256: hash(prompt), tools_sha256: hash(toolJson),
              loadout: host.loadout, network: network.ledger() });
          };
          capture("startup");
          if (resource === "configured" && enabled.includes("mcp.enabled")) {
            fixtureTransport().script([{ toolCalls: [{ callId: "activate", name: "mcp_activate",
              arguments: { server: "docs", tools: [mcpToolName("docs", "lookup")] } }] }, { text: "Finished" }]);
            const ended = nextEvent(events, event => event.kind === "turn_ended");
            // Fake inference and fake Corral lifecycle reports only; no board mutations.
            await host.session.prompt("Activate the fixture read tool.");
            await ended;
            capture("activated");
          }
          console.log(JSON.stringify({ key, loadout: host.loadout }));
        } finally { await host.close(); }
      }
    }
  }
  save("mcp-calls.json", peer.calls);
} finally {
  globalThis.fetch = originalFetch;
  await peer.close();
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
}
