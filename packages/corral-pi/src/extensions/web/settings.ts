import { z } from "zod";

/** Rust resolves defaults once per generation; the host only parses the supplied settings. */
export const WebSettingsSchema = z.object({
  hosts: z.array(z.string().min(1)).readonly(),
  max_bytes: z.number().int().positive().max(Number.MAX_SAFE_INTEGER),
  timeout_ms: z.number().int().positive().max(2147483647),
  max_redirects: z.number().int().min(0).max(10),
  searxng_url: z.string().url().refine((value) => {
    const url = URL.parse(value);
    return url !== null && ["http:", "https:"].includes(url.protocol) && !url.username && !url.password && !url.hash;
  }).optional(),
}).strict().readonly();

export type WebSettings = z.infer<typeof WebSettingsSchema>;
