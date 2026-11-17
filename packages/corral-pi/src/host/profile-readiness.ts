// Offline catalog and credential presence preflight. In particular, an unsupported API
// model must fail before a profile handoff can freeze a paid lineage on that model.
import { getSupportedThinkingLevels } from "@earendil-works/pi-ai/models";
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { CONNECTIONS, isApiConnection, type ConnectionId } from "../auth/connections.ts";
import { PROVIDERS } from "../auth/providers.ts";
import type { AuthModule } from "../auth/types.ts";
import { createModelCatalog, NO_CATALOG_CREDENTIALS } from "./catalog.ts";

export class ProfileReadiness {
  private readonly catalogs = new Map<ConnectionId, Promise<ModelRuntime>>();
  constructor(private readonly auth: AuthModule) {}
  async check(route: { readonly connection: ConnectionId; readonly model: string; readonly effort?: string }): Promise<void> {
    const { connection } = route;
    const provider = CONNECTIONS[connection].provider;
    let catalog = this.catalogs.get(connection);
    if (!catalog) {
      catalog = createModelCatalog(NO_CATALOG_CREDENTIALS, { providers: [provider],
        ...(isApiConnection(connection) ? { apiConnection: connection, modelsPath: null } : {}) });
      this.catalogs.set(connection, catalog);
    }
    const models = await catalog;
    const id = route.model.startsWith(`${provider}/`) ? route.model.slice(provider.length + 1) : route.model;
    const model = models.getModel(provider, id);
    if (!model || model.api !== PROVIDERS[provider].modelApi) {
      throw new Error(`model_unavailable: ${provider}/${id} is not in the installed catalog`);
    }
    if (route.effort && !getSupportedThinkingLevels(model).some(level => level === route.effort)) {
      throw new Error(`model_unavailable: ${route.effort} effort is unsupported for ${provider}/${id}`);
    }
    if (PROVIDERS[provider].auth === "aws-profile" || PROVIDERS[provider].auth === "local") return;
    const credentials = this.auth.credentials({ provider, connection,
      apiEndpoint: PROVIDERS[provider].api, refreshEndpoint: isApiConnection(connection) ? "" : PROVIDERS[provider].refresh });
    const credential = await credentials.read(provider);
    if (credential?.type !== (isApiConnection(connection) ? "api_key" : "oauth")) {
      throw new Error(`credentials_unavailable: run corral agent login --provider ${isApiConnection(connection) ? connection : provider}`);
    }
  }
}
