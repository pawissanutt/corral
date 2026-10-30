import { z } from "zod";

/** Rust validates and canonicalizes these once per generation; the host only parses them. */
export const BrowserSettingsSchema = z.object({
  origins: z.array(z.string().regex(/^https?:\/\/[^/?#@\s]+$/)).readonly(),
  timeout_ms: z.number().int().positive().max(600_000),
  executable: z.string().startsWith("/").optional(),
}).strict().readonly();

export type BrowserSettings = z.infer<typeof BrowserSettingsSchema>;
