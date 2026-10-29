import { z } from "zod";

import type { DestinationRule } from "./providers.ts";

/** An explicit inference origin, never an ambient LLAMA_BASE_URL or a download manager. */
export const LlamaCppSettingsSchema = z.strictObject({
  endpoint: z.string().refine((value) => {
    const url = URL.parse(value);
    return url !== null && ["http:", "https:"].includes(url.protocol) &&
      !url.username && !url.password && !url.search && !url.hash && url.pathname === "/" &&
      !/[\s\\?#]/.test(value) && /^https?:\/\/[^/]+\/?$/.test(value);
  }, "endpoint must be an HTTP(S) origin without credentials, query or fragment"),
  discover: z.boolean(),
  context_window: z.number().int().positive().max(4_294_967_295),
  max_tokens: z.number().int().positive().max(4_294_967_295),
  timeout_ms: z.number().int().positive().max(2_147_483_647),
}).refine((settings) => settings.max_tokens <= settings.context_window,
  "max_tokens must not exceed context_window");

export type LlamaCppSettings = Readonly<z.infer<typeof LlamaCppSettingsSchema>>;

/** Only catalog reads and chat inference; router load/download and foreign origins stay denied. */
export function localInferenceRules(settings: LlamaCppSettings): readonly DestinationRule[] {
  const origin = new URL(settings.endpoint).origin;
  return [
    ...(settings.discover ? [{ method: "GET" as const, path: "/v1/models" }] : []),
    { method: "POST" as const, path: "/v1/chat/completions" },
  ].map(({ method, path }) => ({
    method, display: `${origin}${path}`,
    matches(value, headers) {
      const url = URL.parse(value);
      const authorization = headers.get("authorization");
      return url !== null && url.origin === origin && url.pathname === path && !url.search && !url.hash &&
        !url.username && !url.password && !headers.has("cookie") && !headers.has("x-api-key") &&
        (authorization === null || authorization === "Bearer local");
    },
  }));
}
