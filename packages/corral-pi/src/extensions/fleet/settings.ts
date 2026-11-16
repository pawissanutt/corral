import { z } from "zod";
import type { ChildKind } from "../../build/types.ts";
import type { Role } from "../../roles/types.ts";
import { ExploreSettingsSchema } from "../subagent/settings.ts";

const FleetSettingsSchema = z.object({
  max_parallel: z.number().int().min(1).max(32).default(3),
  max_turns: z.number().int().min(1).max(200).default(60),
  timeout_secs: z.number().int().min(60).max(7200).default(1800),
}).strict();

export function boundFleetSettings(role: Role, enabled: readonly string[], settings?: Readonly<Record<string, unknown>>): {
  allowedKinds: readonly ChildKind[]; maxParallel: number; maxTurns: number; timeoutSecs: number;
} {
  const allowedKinds: ChildKind[] = [];
  if (enabled.includes("subagents")) allowedKinds.push("explore");
  if (enabled.includes("fleet")) {
    if (role === "worker") allowedKinds.push("slice", "review");
    else if (role === "assistant") allowedKinds.push("review");
  }
  const limits = enabled.includes("fleet")
    ? FleetSettingsSchema.parse(settings?.fleet ?? {})
    : enabled.includes("subagents")
      ? ExploreSettingsSchema.parse(settings?.subagents)
      : FleetSettingsSchema.parse({});
  return { allowedKinds, maxParallel: limits.max_parallel, maxTurns: limits.max_turns, timeoutSecs: limits.timeout_secs };
}
