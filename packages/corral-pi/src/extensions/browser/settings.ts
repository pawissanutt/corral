import { z } from "zod";

/** Configuration is validated and canonicalized before the host parses it. */
export const BrowserSettingsSchema = z.object({
  origins: z.array(z.string().regex(/^https?:\/\/[^/?#@\s]+$/)).readonly(),
  timeout_ms: z.number().int().positive().max(600_000),
  executable: z.string().startsWith("/").optional(),
}).strict().readonly();

export type BrowserSettings = z.infer<typeof BrowserSettingsSchema>;
