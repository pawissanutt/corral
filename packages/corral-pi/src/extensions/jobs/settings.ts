import { z } from "zod";

/** Rust resolves these bounds once per generation; the host only parses the supplied values. */
export const JobSettingsSchema = z.object({
  max_jobs: z.number().int().min(1).max(16),
  max_output_bytes: z.number().int().min(1024).max(1024 * 1024 * 1024),
  max_runtime_secs: z.number().int().min(1).max(7 * 24 * 3600),
}).strict().readonly();

export type JobSettings = z.infer<typeof JobSettingsSchema>;
