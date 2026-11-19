import type { Api, Model } from "@earendil-works/pi-ai";
import type { AgentSession, ModelRuntime } from "@earendil-works/pi-coding-agent";
import { PROVIDERS, billingKind, isProviderId } from "../auth/providers.ts";
import { resolveClaudeCode } from "../claude/sdk.ts";

/** Explicit selection and fallback share preflight and Pi's journaled commit. */
export function installModelSelection(session: AgentSession, models: ModelRuntime, interactive: boolean) {
  const setModel = session.setModel.bind(session);
  const switchModel = async (model: Model<Api>, options?: Parameters<AgentSession["setModel"]>[1],
    automatic = false, signal?: AbortSignal) => {
    const idle = () => {
      if (!automatic && session.isStreaming) throw new Error("model_switch_refused: wait for the current turn to finish; current model unchanged");
    };
    idle();
    const canonical = models.getModel(model.provider, model.id);
    if (!isProviderId(model.provider) || !canonical || canonical.api !== model.api || model.api !== PROVIDERS[model.provider].modelApi
      || (!automatic && !interactive && model.provider !== session.model?.provider)) {
      throw new Error(`model_unavailable: ${model.provider}/${model.id}; current model unchanged`);
    }
    if (PROVIDERS[model.provider].lane) resolveClaudeCode();
    // getAuth refreshes expired OAuth credentials before any session/model-change entry is written.
    if (!await models.getAuth(canonical, { signal })) {
      throw new Error(`credentials_unavailable: run \`corral-pi login --provider ${model.provider}\`; current model unchanged`);
    }
    signal?.throwIfAborted();
    idle();
    await setModel(canonical, options);
  };
  session.setModel = (model, options) => switchModel(model, options);
  session.cycleModel = async (direction = "forward", options = {}) => {
    const current = session.model;
    if (!current || !isProviderId(current.provider)) return undefined;
    const kind = billingKind(current.provider);
    const available = (await models.getAvailable(interactive ? undefined : current.provider))
      .filter(model => isProviderId(model.provider) && billingKind(model.provider) === kind);
    if (available.length <= 1 || session.model !== current) return undefined;
    const index = available.findIndex(m => m.provider === current.provider && m.id === current.id);
    const next = available[(Math.max(0, index) + (direction === "forward" ? 1 : available.length - 1)) % available.length];
    if (!next) return undefined;
    await session.setModel(next, options);
    return { model: next, thinkingLevel: session.thinkingLevel, isScoped: false };
  };
  return (model: Model<Api>, signal?: AbortSignal) => switchModel(model, undefined, true, signal);
}
