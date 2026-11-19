import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { serverVisible } from "../../host/mcp-settings.ts";
import { loadModules, modulePaths } from "../../host/modules.ts";
import { PACKAGE_ROOT } from "../../../test/support/fixture-install.ts";
import { REQUEST_ASSET, REQUEST_OFF_ASSET } from "../common/todo.ts";
import { askUserContract } from "../common/ask-user.ts";
import type { HostIdentity } from "../types.ts";
import assistant, { CONTRACT_ASSET, UPSTREAM, assistantRole } from "./index.ts";
import { utf8Prefix } from "./owner-action.ts";

const identity: HostIdentity = { store: "st-0123456789abcdef", project: "", role: "assistant", assistant: "A-3" };
const context = { identity, cwd: "/work", model: "m" };
test("the assistant can read, shell and write, and offers local todo only when enabled", () => {
  expect(assistant.role).toBe("assistant");
  expect(assistant.upstreamTools).toEqual(UPSTREAM);
  expect(assistant.upstreamTools).toContain("edit");
  expect(assistant.upstreamTools).toContain("write");
  expect(assistant.upstreamTools).toContain("bash");
  expect(assistant.tools(context)).toEqual([]);
  expect(assistant.tools({ ...context, todo: true }).map((tool) => tool.name)).toEqual(["corral_todo"]);
});

test("the contract names the bound session and leaves no placeholder", () => {
  const contract = assistantRole(PACKAGE_ROOT).contract(context);
  expect(contract).toContain("corral assistant session A-3");
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

test("a workspace-scoped assistant sees every configured MCP server; bound roles see their project's", () => {
  const server = { projects: ["app"], args: [], tools: {}, resources: [], url: "https://mcp.example" };
  expect(serverVisible(server, "assistant", "")).toBeTrue();
  expect(serverVisible(server, "worker", "app")).toBeTrue();
  expect(serverVisible(server, "worker", "web")).toBeFalse();
});

test("owner text truncation preserves UTF-8 boundaries", () => {
  expect(utf8Prefix("ab\u00e9", 3)).toBe("ab");
  expect(utf8Prefix("abc", 3)).toBe("abc");
});

for (const todo of [true, false, undefined] as const) {
  test(`the loaded assistant contract equals the shipped assets with its bound identity (todo=${String(todo)})`, async () => {
    // Given the independently read shipped assets for this capability setting and bound session.
    const configured = todo === undefined ? context : { ...context, todo };
    const shipped = readFileSync(join(PACKAGE_ROOT, CONTRACT_ASSET), "utf8");
    const execution = readFileSync(join(PACKAGE_ROOT, todo === true ? REQUEST_ASSET : REQUEST_OFF_ASSET), "utf8");
    const expected = `${shipped.replaceAll("{{assistant}}", identity.assistant ?? "")}\n\n${execution}\n\n${askUserContract(configured)}`;
    // When the production fixed-path loader and the explicit-root role build their contracts.
    const loaded = await loadModules(PACKAGE_ROOT, "assistant");
    // Then both equal the complete shipped copy, including identity and the selected request section.
    expect(loaded.role.contract(configured)).toBe(expected);
    expect(assistantRole(PACKAGE_ROOT).contract(configured)).toBe(expected);
  });
}

