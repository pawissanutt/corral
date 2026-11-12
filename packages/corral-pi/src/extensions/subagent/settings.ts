import { z } from "zod";
import type { Role } from "../../roles/types.ts";

const model = z.string().min(1).max(256).nullable();
const timeout_secs = z.number().int().min(1).max(600);
export const ExploreSettingsSchema = z.object({
  max_parallel: z.number().int().min(1).max(16), model,
  max_turns: z.number().int().min(1).max(64), timeout_secs,
  service_tier: z.enum(["default", "priority"]).optional(),
}).strict();
export const OracleSettingsSchema = z.object({ model, timeout_secs }).strict();
export const ShadowSettingsSchema = z.object({
  model: z.string().min(1).max(256), timeout_secs, max_per_hour: z.number().int().min(1).max(600),
}).strict();
export const ExpertSettingsSchema = z.object({
  model: z.string().min(1).max(256), max_turns: z.number().int().min(1).max(64),
  timeout_secs, max_parallel: z.number().int().min(1).max(16),
}).strict();
export type ExpertSettings = z.infer<typeof ExpertSettingsSchema>;
export type ShadowSettings = z.infer<typeof ShadowSettingsSchema>;
export type ExploreSettings = z.infer<typeof ExploreSettingsSchema>;
export type OracleSettings = z.infer<typeof OracleSettingsSchema>;
export interface AuxiliarySettings {
  readonly explore?: ExploreSettings;
  readonly oracle?: OracleSettings;
  readonly web_search?: { readonly model: null; readonly timeout_secs: number };
  readonly shadow?: ShadowSettings;
  readonly expert?: ExpertSettings;
}

export function auxiliarySettings(role: Role, enabled: readonly string[], settings?: Readonly<Record<string, unknown>>): AuxiliarySettings {
  return {
    ...(enabled.includes("subagents") ? { explore: ExploreSettingsSchema.parse(settings?.subagents) } : {}),
    ...(enabled.includes("oracle") ? { oracle: OracleSettingsSchema.parse(settings?.oracle) } : {}),
    ...(role === "pm" && enabled.includes("shadow") ? { shadow: ShadowSettingsSchema.parse(settings?.shadow) } : {}),
    ...(role === "pm" && enabled.includes("expert") ? { expert: ExpertSettingsSchema.parse(settings?.expert) } : {}),
  };
}
