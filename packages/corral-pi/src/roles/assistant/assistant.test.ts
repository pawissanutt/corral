import { expect, test } from "bun:test";

import { LaunchMessageInvalid, parseLaunchMessage } from "../../host/config.ts";
import { serverVisible } from "../../host/mcp-settings.ts";
import { loadModules, modulePaths } from "../../host/modules.ts";
import { CorralClient } from "../../tools/corral.ts";
import { snapshotProgress } from "../../ui/todo.ts";
import { PACKAGE_ROOT } from "../../../test/support/fixture-install.ts";
import type { HostIdentity } from "../types.ts";
import { GENERATION_ENV, SESSION_ENV, exportAssistantSession } from "./env.ts";
import assistant, { UPSTREAM, assistantRole } from "./index.ts";

const identity: HostIdentity = { store: "st-0123456789abcdef", project: "", role: "assistant", assistant: "A-3", generation: 2 };
const context = { identity, cwd: "/work", model: "m" };
const message = {
  version: 1, store: "st-demo", project: "", role: "assistant", assistant: "A-1", generation: 1,
  secrets: { host_cap: "h".repeat(64), ctl_cap: "c".repeat(64), bind_nonce: "n".repeat(64) },
  pane: "p2", cwd: "/work", root: "/work", corral: "/bin/corral",
  assets: "/assets", runtime_dir: "/runtime", session_dir: "/sessions", model: "gpt-6.1-sol",
};

test("the assistant loadout reads, shells and writes behind its path guard, and offers todo only when enabled", () => {
  expect(assistant.role).toBe("assistant");
  expect(assistant.upstreamTools).toEqual(UPSTREAM);
  expect(assistant.upstreamTools).toContain("edit");
  expect(assistant.upstreamTools).toContain("write");
  expect(typeof assistant.extend).toBe("function");
  expect(assistant.corralOps).toEqual(["hello", "event", "receipt_lookup", "todo"]);
  expect(assistant.tools(context)).toEqual([]);
  expect(assistant.tools({ ...context, todo: true }).map((tool) => tool.name)).toEqual(["corral_todo"]);
});

test("the contract names the bound session and leaves no placeholder", () => {
  const contract = assistantRole(PACKAGE_ROOT).contract(context);
  expect(contract).toContain("corral assistant session A-3");
  expect(contract).toContain("CORRAL_ASSISTANT=A-3");
  expect(contract).not.toContain("{{");
});

test("the fixed-path loader finds the assistant role module", async () => {
  expect(modulePaths("assistant").role).toBe("src/roles/assistant/index.ts");
  const loaded = await loadModules(PACKAGE_ROOT, "assistant");
  expect(loaded.role.role).toBe("assistant");
});

test("an assistant launch names its session and no project; no other role names one", () => {
  const parsed = parseLaunchMessage(JSON.stringify(message));
  expect([parsed.identity.role, parsed.identity.assistant, parsed.identity.project]).toEqual(["assistant", "A-1", ""]);
  const reviewer = { ...message, assistant: undefined, role: "reviewer", project: "demo", task: "T-001", run: "R1", review: "R1-r", callback: "g1:c1" };
  for (const invalid of [
    { ...message, assistant: undefined },
    { ...message, assistant: "A-0" },
    { ...message, assistant: "B-1" },
    { ...message, project: "demo" },
    { ...reviewer, assistant: "A-1" },
  ]) {
    expect(() => parseLaunchMessage(JSON.stringify(invalid))).toThrow(LaunchMessageInvalid);
  }
  expect(parseLaunchMessage(JSON.stringify(reviewer)).identity.assistant).toBeUndefined();
});

test("the host exports the session for its shell and clears it for every other role", () => {
  const env: NodeJS.ProcessEnv = { [SESSION_ENV]: "A-9", [GENERATION_ENV]: "9" };
  exportAssistantSession(env, identity);
  expect([env[SESSION_ENV], env[GENERATION_ENV]]).toEqual(["A-3", "2"]);
  exportAssistantSession(env, { store: "st-0", project: "demo", role: "pm", generation: 1 });
  expect(SESSION_ENV in env || GENERATION_ENV in env).toBeFalse();
});

test("envelopes carry the session, and the todo readback is the session's container", () => {
  const client = new CorralClient({ corral: "/bin/corral", root: "/work", identity, hostCap: "h".repeat(64), sessionId: () => "s" });
  const envelope = JSON.parse(client.envelope("todo", { requestId: "rq-1", payload: {} }));
  const { role, assistant: session, project, generation } = envelope.identity;
  expect([role, session, project, generation]).toEqual(["assistant", "A-3", "", 2]);
  expect(snapshotProgress({ container: { state: { todo: null } } }, identity)).toBeNull();
  expect(snapshotProgress({ container: { state: {} } }, identity)).toBeNull();
});

test("a workspace-scoped assistant sees every configured MCP server; bound roles see their project's", () => {
  const server = { projects: ["app"], args: [], tools: {}, resources: [], url: "https://mcp.example" };
  expect(serverVisible(server, "assistant", "")).toBeTrue();
  expect(serverVisible(server, "worker", "app")).toBeTrue();
  expect(serverVisible(server, "worker", "web")).toBeFalse();
  expect(serverVisible(server, "pm", "")).toBeFalse();
});
