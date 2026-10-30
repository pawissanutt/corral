// Bash rules and hooks around tool execution (T-572). They are registered after the host's
// execution check, so a denied tool never reaches them. Bash rules decide every bash call,
// nested ones too; hooks wrap only top-level local calls, never corral operations, denied
// calls or calls another tool issued.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import type { EventLog } from "../bridge/events.ts";
import {
  type BashPermissions, type BashRuleSettings, BashRuleSettingsSchema, decideBash,
} from "../capabilities/bash-rules.ts";
import { type HookSettings, HookSettingsSchema, type Hooks } from "../capabilities/hooks.ts";
import type { Role } from "../roles/types.ts";

export interface HostControls {
  readonly hooks?: HookSettings;
  readonly bashRules?: BashRuleSettings;
}

/** Roles each control may serve; the Rust catalog applies the same ceilings. */
const CONTROL_ROLES: Readonly<Record<"hooks" | "bash_rules", readonly Role[]>> = {
  hooks: ["worker"],
  bash_rules: ["worker", "reviewer", "assistant"],
};

/** The enabled controls of a generation, from its snapshot's resolved settings; throws when malformed. */
export function parseControls(
  role: Role, enabled: readonly string[], settings: Readonly<Record<string, unknown>> | undefined,
): HostControls {
  const pick = <T>(key: "hooks" | "bash_rules", schema: { safeParse(value: unknown): { success: true; data: T } | { success: false } }) => {
    if (!enabled.includes(key)) return undefined;
    if (!CONTROL_ROLES[key].includes(role)) throw new Error(`${key} is not available to the ${role} role`);
    const parsed = schema.safeParse(settings?.[key]);
    if (!parsed.success) throw new Error(`capabilities.settings.${key} is missing or malformed`);
    return parsed.data;
  };
  const hooks = pick("hooks", HookSettingsSchema);
  const bashRules = pick("bash_rules", BashRuleSettingsSchema);
  return { ...(hooks ? { hooks } : {}), ...(bashRules ? { bashRules } : {}) };
}

export interface ToolControls {
  readonly hooks?: Hooks;
  readonly rules?: BashRuleSettings;
  readonly permissions?: BashPermissions;
  readonly events: EventLog;
  readonly corral: ReadonlySet<string>;
  currentTurn(): number | undefined;
}

/** Tools whose `command` input runs in a shell: background jobs obey the same bash rules (T-565). */
const SHELL_TOOLS: ReadonlySet<string> = new Set(["bash", "job_start"]);

export function installToolControls(pi: ExtensionAPI, controls: ToolControls): void {
  const hooked = new Set<string>();
  pi.on("tool_call", async (event) => {
    const nested = event.parentToolCallId !== undefined;
    if (controls.rules && SHELL_TOOLS.has(event.toolName)) {
      const input = event.input as Record<string, unknown>;
      const command = typeof input.command === "string" ? input.command : "";
      const decision = decideBash(command, controls.rules);
      const detail = { tool_call_id: event.toolCallId, ...(decision.rule ? { rule: decision.rule } : {}),
        ...(nested ? { nested: true } : {}) };
      if (decision.action === "deny") {
        controls.events.emit("bash_rule_denied", detail);
        return { block: true, reason: `bash rule ${JSON.stringify(decision.rule ?? "default deny")} denies `
          + `${JSON.stringify(decision.segment ?? command)}; nothing ran` };
      }
      if (decision.action === "ask") {
        if (!controls.permissions?.consume(command)) {
          controls.events.emit("bash_permission_required", detail);
          return { block: true, reason: controls.permissions?.request(command, decision)
            ?? "bash rule requires owner permission; nothing ran" };
        }
        controls.events.emit("bash_permission_used", detail);
      }
    }
    const hooks = controls.hooks;
    if (!hooks || nested || controls.corral.has(event.toolName) || !hooks.wraps(event.toolName)) return undefined;
    const refusal = await hooks.before({
      tool: event.toolName, toolCallId: event.toolCallId, input: event.input,
      ...(controls.currentTurn() !== undefined ? { turn: controls.currentTurn() } : {}),
    });
    if (refusal) return { block: true, reason: refusal };
    hooked.add(event.toolCallId);
    return undefined;
  });
  pi.on("tool_result", async (event) => {
    if (event.toolName === "corral_ask" && event.parentToolCallId === undefined && controls.permissions) {
      if (controls.permissions.observeAsk(event.input, event.details)) {
        controls.events.emit("bash_permission_asked", { tool_call_id: event.toolCallId });
      }
    }
    if (!controls.hooks || !hooked.delete(event.toolCallId)) return undefined;
    const notes = await controls.hooks.after({
      tool: event.toolName, toolCallId: event.toolCallId, input: event.input,
      ...(controls.currentTurn() !== undefined ? { turn: controls.currentTurn() } : {}),
    }, event.isError);
    return notes.length === 0 ? undefined : { content: [...event.content, { type: "text", text: notes.join("\n") }] };
  });
  if (controls.hooks) {
    const hooks = controls.hooks;
    pi.on("agent_settled", async () => {
      await hooks.turnEnd(controls.currentTurn());
    });
  }
}
