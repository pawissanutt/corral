// Opt-in Codex transport experiment. Session identity and non-conversation requests stay untouched.
import { createHash } from "node:crypto";
import type { StreamFunction } from "@earendil-works/pi-ai";

const KEY_VERSION = "corral-pm-cache-key-v1";

export function pmCacheKeyStream(stream: StreamFunction, options: {
  mode: "session" | "stable";
  identity: { store: string; role: string };
  sessionId(): string;
}): StreamFunction {
  if (options.mode !== "stable" || options.identity.role !== "pm") return stream;
  return (model, context, request) => {
    // The main Pi agent loop forwards convertToLlm. Summarization omits it, and
    // auxiliary loops carry their own session id. Match the usage guard's classification.
    if (model.api !== "openai-codex-responses" || model.provider !== "openai-codex"
      || !request || !("convertToLlm" in request) || typeof request.convertToLlm !== "function"
      || request.sessionId !== options.sessionId() || request.cacheRetention === "none") {
      return stream(model, context, request);
    }
    const key = createHash("sha256").update(JSON.stringify([
      options.identity.store, "pm", model.provider, model.id, KEY_VERSION,
    ])).digest("hex");
    return stream(model, context, { ...request, onPayload: async (payload, current) => {
      const amended = await request.onPayload?.(payload, current);
      const body = amended === undefined ? payload : amended;
      if (typeof body !== "object" || body === null || Array.isArray(body)) return body;
      return { ...body, prompt_cache_key: key };
    } });
  };
}
