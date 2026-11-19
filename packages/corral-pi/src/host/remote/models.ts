import type { AgentSession, ModelRuntime } from "@earendil-works/pi-coding-agent";
import { getSupportedThinkingLevels } from "@earendil-works/pi-ai/models";
import type { RemoteRegistry } from "./registry.ts";
import { clipText, fingerprintPart, isoTime, jsonBytes, pageRows, parseCursor } from "./state.ts";

interface ModelChoice { id: string; provider: string; name?: string; context_window?: number; effort_levels: readonly string[]; scoped: boolean; note?: string }
export interface SessionModels {
  current?: string; default?: string; effort?: string; models: ModelChoice[]; refreshed_at?: string; refreshing: boolean;
}

export class ModelCatalogStatus {
  refreshedAt: string | undefined;
  private inflight = 0;
  get refreshing(): boolean { return this.inflight > 0; }

  observe(runtime: ModelRuntime, changed: () => void): void {
    this.refreshedAt = isoTime(Date.now());
    const refresh = runtime.refresh.bind(runtime);
    runtime.refresh = async options => {
      this.inflight++;
      changed();
      try {
        const result = await refresh(options);
        if (!runtime.getError()) this.refreshedAt = isoTime(Date.now());
        return result;
      } finally {
        this.inflight--;
        changed();
      }
    };
  }
}

export function modelsPart(session: Pick<AgentSession, "model" | "thinkingLevel" | "modelRuntime" | "scopedModels">,
  defaultModel: string, interactive: boolean, catalog: ModelCatalogStatus): SessionModels {
  const selected = session.model;
  const current = selected ? `${selected.provider}/${selected.id}` : undefined;
  const scoped = new Set(session.scopedModels.map(({ model }) => `${model.provider}/${model.id}`));
  const seen = new Set<string>(), models: ModelChoice[] = [];
  for (const model of session.modelRuntime.getModels(interactive ? undefined : selected?.provider)) {
    const id = `${model.provider}/${model.id}`;
    if (seen.has(id) || Buffer.byteLength(id) > 512) continue;
    seen.add(id);
    models.push({ id, provider: model.provider, name: clipText(model.name, 256), context_window: model.contextWindow,
      effort_levels: model.reasoning ? getSupportedThinkingLevels(model) : [], scoped: scoped.has(id) });
  }
  models.sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
  return { current, default: defaultModel, effort: selected?.reasoning ? session.thinkingLevel : undefined,
    models, refreshed_at: catalog.refreshedAt, refreshing: catalog.refreshing };
}

export function registerModels(registry: RemoteRegistry, read: () => SessionModels): void {
  registry.register({ name: "models", parse: ({ after }) => parseCursor(after), run: after => {
    const { models, ...meta } = read();
    const { items, next } = pageRows(models, after, fingerprintPart(["models", meta, models]), Infinity, jsonBytes({ ...meta, models: [] }));
    return { ...meta, models: items, next };
  } });
}
