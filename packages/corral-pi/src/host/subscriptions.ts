// Discover only corral-owned subscription logins; never ambient API keys or metered routes.
import type { CredentialStore } from "@earendil-works/pi-ai";
import type { AgentSession, ModelRuntime } from "@earendil-works/pi-coding-agent";
import { PROVIDER_IDS, PROVIDERS, type ProviderId } from "../auth/providers.ts";
import type { AuthModule } from "../auth/types.ts";
import { resolveClaudeCode } from "../claude/sdk.ts";
import { NO_CATALOG_CREDENTIALS } from "./catalog.ts";

export async function subscriptionRoutes(auth: AuthModule, primary: ProviderId, role: string) {
  if (PROVIDERS[primary].auth === "local") return { providers: [primary], credentials: NO_CATALOG_CREDENTIALS };
  const context = (provider: ProviderId) => ({ provider, apiEndpoint: PROVIDERS[provider].api,
    refreshEndpoint: PROVIDERS[provider].refresh });
  const stores = new Map<string, CredentialStore>();
  stores.set(primary, auth.credentials(context(primary)));
  if ((role === "assistant" || role === "pm") && PROVIDER_IDS.includes(primary)) {
    for (const id of PROVIDER_IDS) {
      if (id === primary || !(auth.providers ?? [auth.provider]).includes(id)) continue;
      try {
        const store = auth.credentials(context(id));
        if ((await store.read(id))?.type === "oauth") stores.set(id, store);
      } catch {
        // An absent/unusable optional login does not prevent the current route from working.
      }
    }
  }
  const credentials: CredentialStore = {
    read: async (id, options) => stores.get(id)?.read(id, options),
    list: async (options) => (await Promise.all([...stores].map(async ([id, store]) =>
      (await store.list(options)).filter((entry) => entry.providerId === id)))).flat(),
    modify: async (id, fn, options) => {
      const store = stores.get(id);
      if (!store) throw new Error(`provider_not_allowed: ${id}`);
      return store.modify(id, fn, options);
    },
    delete: async (id, options) => { await stores.get(id)?.delete(id, options); },
  };
  return { providers: [...stores.keys()] as ProviderId[], credentials };
}

/** Pi's picker and model cycling both commit through this preflight, not a post-change hook. */
export function installModelSelection(session: AgentSession, models: ModelRuntime, providers: readonly ProviderId[]): void {
  const setModel = session.setModel.bind(session);
  session.setModel = async (model, options) => {
    if (session.isStreaming) throw new Error("model_switch_refused: wait for the current turn to finish; current model unchanged");
    const route = PROVIDERS[model.provider as ProviderId];
    const canonical = models.getModel(model.provider, model.id);
    if (PROVIDER_IDS.includes(model.provider as ProviderId) && !providers.includes(model.provider as ProviderId)) {
      throw new Error(`credentials_unavailable: ${model.provider} was not approved at startup; run ` +
        `\`corral agent login --provider ${model.provider}\` and start a new session; current model unchanged`);
    }
    if (!providers.includes(model.provider as ProviderId) || !route || !canonical || canonical.api !== model.api ||
        model.api !== route.modelApi) {
      throw new Error(`model_unavailable: ${model.provider}/${model.id}; current model unchanged`);
    }
    if (route.lane) resolveClaudeCode();
    // getAuth refreshes expired OAuth credentials before any session/model-change entry is written.
    if (!await models.getAuth(canonical)) {
      throw new Error(`credentials_unavailable: run \`corral agent login --provider ${model.provider}\`; current model unchanged`);
    }
    await setModel(canonical, options);
  };
  session.cycleModel = async (direction = "forward", options = {}) => {
    const available = await models.getAvailable();
    if (available.length <= 1) return undefined;
    const index = available.findIndex((m) => m.provider === session.model?.provider && m.id === session.model.id);
    const next = available[(Math.max(0, index) + (direction === "forward" ? 1 : available.length - 1)) % available.length];
    if (!next) return undefined;
    await session.setModel(next, options);
    return { model: next, thinkingLevel: session.thinkingLevel, isScoped: false };
  };
}
