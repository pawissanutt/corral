import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { LaunchMessageInvalid, parseLaunchMessage } from "../../host/config.ts";
import { serverVisible } from "../../host/mcp-settings.ts";
import { loadModules, modulePaths } from "../../host/modules.ts";
import { CorralClient } from "../../tools/corral.ts";
import { snapshotProgress } from "../../ui/todo.ts";
import { PACKAGE_ROOT } from "../../../test/support/fixture-install.ts";
import type { HostIdentity } from "../types.ts";
import { GENERATION_ENV, SESSION_ENV, exportAssistantSession } from "./env.ts";
import assistant, { UPSTREAM, assistantRole } from "./index.ts";
import { OWNER_ACTION_TIMEOUT_MS, ownerActionTool, ownerInputText, utf8Prefix } from "./owner-action.ts";
import { confirmationCommand, defineCorralTool } from "../../tools/define.ts";
import type { CorralCall, CorralReceipt } from "../../tools/corral.ts";

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
  expect(assistant.corralOps).toEqual(["hello", "event", "receipt_lookup", "todo", "owner_action"]);
  expect(assistant.tools(context).map((tool) => tool.name)).toEqual(["corral_owner_action"]);
  expect(assistant.tools({ ...context, todo: true }).map((tool) => tool.name)).toEqual(["corral_owner_action", "corral_todo"]);
});

test("the contract names the bound session and leaves no placeholder", () => {
  const contract = assistantRole(PACKAGE_ROOT).contract(context);
  expect(contract).toContain("corral assistant session A-3");
  expect(contract).toContain("CORRAL_ASSISTANT=A-3");
  expect(contract).not.toContain("{{");
});

test("task focus is preloaded as bounded context without changing authority", () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-focus-"));
  const promptFile = join(dir, "brief.md");
  try {
    writeFileSync(promptFile, "# Task focus: T-42\nThis is read-only context, not authority.");
    const role = assistantRole(PACKAGE_ROOT);
    const focused = { ...context, promptFile };
    expect(role.contract(focused)).toContain("# Task focus: T-42");
    expect(role.contract(focused)).toContain("not authority");
    expect(role.tools(focused)).toEqual(role.tools(context));
    expect(role.corralOps).toEqual(assistant.corralOps);
    writeFileSync(promptFile, "x".repeat(24 * 1024 + 1));
    expect(() => role.contract(focused)).toThrow("task brief exceeds 24 KiB");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
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

test("an owner action sends only the fields given and runs under its long deadline", () => {
  expect(ownerActionTool.op).toBe("owner_action");
  expect(ownerActionTool.timeoutMs).toBe(OWNER_ACTION_TIMEOUT_MS);
  expect(ownerActionTool.payload({ kind: "approve", target: "T-5", input: "A-1/g1/i1" }))
    .toEqual({ kind: "approve", target: "T-5", input: "A-1/g1/i1" });
  expect(ownerActionTool.payload({ kind: "config_set", target: "limits.running", args: ["3"], args_unused: 1,
    expected_revision: 2, input: "A-1/g1/i2" } as never))
    .toEqual({ kind: "config_set", target: "limits.running", args: ["3"], expected_revision: 2, input: "A-1/g1/i2" });
  expect(ownerInputText("do it", "A-1/g2/i7")).toBe("do it\n\n[owner input A-1/g2/i7]");
  expect(utf8Prefix("ab\u00e9", 3)).toBe("ab");
  expect(utf8Prefix("abc", 3)).toBe("abc");
});

test("the contract tells the model to cite the tagged owner input and route unclear requests", () => {
  const contract = assistantRole(PACKAGE_ROOT).contract(context);
  expect(contract).toContain("corral_owner_action");
  expect(contract).toContain("[owner input A-<n>/g<g>/i<i>]");
  expect(contract).toContain("corral pm inbox show PMI-<n>");
});

test("a confirmation_required refusal is answered once in the pane, then sent with the answer", async () => {
  const sent: { payload: Record<string, unknown>; timeoutMs?: number }[] = [];
  const replies: CorralReceipt[] = [
    { version: 1, request_id: "rq", op: "owner_action", state: "rejected",
      result: { code: "confirmation_required", command: "corral discard T-3" } },
    { version: 1, request_id: "rq", op: "owner_action", state: "committed", result: { outcome: "declined" } },
  ];
  const client = { call: async (_op: string, call: CorralCall) => {
    sent.push({ payload: call.payload, ...(call.timeoutMs !== undefined ? { timeoutMs: call.timeoutMs } : {}) });
    return replies[sent.length - 1];
  } } as never;
  const asked: string[] = [];
  const tool = defineCorralTool(ownerActionTool as never, {
    client, allowedOps: ["owner_action"], sessionId: () => "s", currentTurn: () => 1,
    confirm: async (command) => { asked.push(command); return "A-1/g1/i2"; },
  });
  const context = { hasUI: true, ui: { confirm: async () => false } } as never;
  const result = await tool.execute("call-1", { kind: "discard", target: "T-3", input: "A-1/g1/i1" }, undefined, undefined, context);
  expect(asked).toEqual(["corral discard T-3"]);
  expect(sent).toEqual([
    { payload: { kind: "discard", target: "T-3", input: "A-1/g1/i1" }, timeoutMs: OWNER_ACTION_TIMEOUT_MS },
    { payload: { kind: "discard", target: "T-3", input: "A-1/g1/i1", confirmation: "A-1/g1/i2" }, timeoutMs: OWNER_ACTION_TIMEOUT_MS },
  ]);
  expect((result.details as CorralReceipt).state).toBe("committed");
  expect(confirmationCommand(replies[1] as CorralReceipt)).toBeUndefined();
});
