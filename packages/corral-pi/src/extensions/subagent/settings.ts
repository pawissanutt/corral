import { z } from "zod";
import type { Role } from "../../roles/types.ts";

const model = z.string().min(1).max(256).nullable();
const timeout_secs = z.number().int().min(1).max(600);
export const ExploreSettingsSchema = z.object({
  max_parallel: z.number().int().min(1).max(16), model,
  max_turns: z.number().int().min(1).max(64), timeout_secs,
}).strict();
export const OracleSettingsSchema = z.object({ model, timeout_secs }).strict();
export type ExploreSettings = z.infer<typeof ExploreSettingsSchema>;
export type OracleSettings = z.infer<typeof OracleSettingsSchema>;
export interface AuxiliarySettings { readonly explore?: ExploreSettings; readonly oracle?: OracleSettings }

export function auxiliarySettings(role: Role, enabled: readonly string[], settings?: Readonly<Record<string, unknown>>): AuxiliarySettings {
  if (role === "pm" && enabled.includes("subagents")) throw new Error("subagents is not available to PM");
  return {
    ...(enabled.includes("subagents") ? { explore: ExploreSettingsSchema.parse(settings?.subagents) } : {}),
    ...(enabled.includes("oracle") ? { oracle: OracleSettingsSchema.parse(settings?.oracle) } : {}),
  };
}
