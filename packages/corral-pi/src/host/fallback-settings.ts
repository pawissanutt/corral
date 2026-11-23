import { z } from "zod";
import { PROVIDER_IDS, isProviderId, type ProviderId } from "../auth/providers.ts";
import type { ModelChoice } from "./config.ts";
import type { FallbackSelector } from "./continuation.ts";

export type SubscriptionProviderId = "openai-codex" | "anthropic";
export type FallbackTargetProviderId = Exclude<ProviderId, "llama.cpp">;

export interface FallbackSettings {
  readonly children: boolean;
  readonly providers: Readonly<Partial<Record<SubscriptionProviderId, readonly FallbackTargetProviderId[]>>>;
}

export const EMPTY_FALLBACK_SETTINGS: FallbackSettings = { children: false, providers: {} };

const FallbackSchema = z.strictObject({
  children: z.boolean().default(false),
  providers: z.record(z.string(), z.array(z.string())).default({}),
});

function isSubscriptionProvider(provider: string): provider is SubscriptionProviderId {
  return (PROVIDER_IDS as readonly string[]).includes(provider);
}

/** Parse and validate `[fallback]` from `.corral-next/pi.toml`. */
export function parseFallbackSettings(raw: unknown): FallbackSettings {
  if (raw === undefined) return EMPTY_FALLBACK_SETTINGS;
  const parsed = FallbackSchema.safeParse(raw);
  if (!parsed.success) {
    const issues = parsed.error.issues.map(issue => `${["fallback", ...issue.path].join(".")}: ${issue.message}`);
    throw new Error(`.corral-next/pi.toml: ${issues.join("; ")}`);
  }
  const providers: Partial<Record<SubscriptionProviderId, FallbackTargetProviderId[]>> = {};
  for (const [source, targets] of Object.entries(parsed.data.providers)) {
    if (!isSubscriptionProvider(source)) {
      throw new Error(
        `.corral-next/pi.toml: fallback.providers.${source}: only subscription providers (${PROVIDER_IDS.join(", ")}) can define quota fallbacks`,
      );
    }
    const seen = new Set<string>();
    const validated: FallbackTargetProviderId[] = [];
    for (const [index, target] of targets.entries()) {
      if (!isProviderId(target) || target === "llama.cpp") {
        throw new Error(`.corral-next/pi.toml: fallback.providers.${source}.${index}: unsupported fallback provider ${JSON.stringify(target)}`);
      }
      if (target === source) {
        throw new Error(`.corral-next/pi.toml: fallback.providers.${source}.${index}: fallback provider cannot point to itself`);
      }
      if (seen.has(target)) {
        throw new Error(`.corral-next/pi.toml: fallback.providers.${source}.${index}: duplicate fallback provider ${JSON.stringify(target)}`);
      }
      seen.add(target);
      validated.push(target);
    }
    providers[source] = validated;
  }
  return { children: parsed.data.children, providers };
}

export function hasProviderFallbacks(settings?: FallbackSettings): boolean {
  return settings !== undefined && Object.values(settings.providers).some(targets => (targets?.length ?? 0) > 0);
}

/** Resolve ordered fallback selectors for the failing model's subscription provider. */
export function providerFallbackCandidates(
  current: { readonly provider: string; readonly id?: string; readonly model?: string },
  settings?: FallbackSettings,
): FallbackSelector[] {
  const modelId = current.id ?? current.model;
  if (!modelId || !settings || !isSubscriptionProvider(current.provider)) return [];
  const targets = settings.providers[current.provider] ?? [];
  return targets.map(provider => ({ provider, model: modelId }));
}

/** Transitive closure of additional providers reachable from `initial` via `[fallback.providers]`. */
export function fallbackProvidersFor(initial: readonly ProviderId[], settings?: FallbackSettings): ProviderId[] {
  if (!settings || !hasProviderFallbacks(settings)) return [];
  const seeded = new Set<ProviderId>(initial);
  const discovered: ProviderId[] = [];
  const seen = new Set<ProviderId>(initial);
  const queue: ProviderId[] = [...initial];
  while (queue.length > 0) {
    const provider = queue.shift()!;
    if (!isSubscriptionProvider(provider)) continue;
    for (const target of settings.providers[provider] ?? []) {
      if (!seen.has(target)) {
        seen.add(target);
        if (!seeded.has(target)) discovered.push(target);
        queue.push(target);
      }
    }
  }
  return discovered;
}

export interface BranchModelEntry {
  readonly type: string;
  readonly provider?: string;
  readonly modelId?: string;
}

/** Candidate fallback models a non-interactive resume/retry may legitimately restore. */
export function restorableFallbacks(
  launch: ModelChoice | undefined,
  explicit: readonly ModelChoice[] | undefined,
  policy: FallbackSettings | undefined,
  noFallback: boolean | undefined,
  branch: readonly BranchModelEntry[],
): readonly ModelChoice[] {
  if (noFallback) return [];

  const last = branch.findLastIndex(entry => entry.type === "model_change");
  const saved = branch[last];
  if (!saved || !isProviderId(saved.provider) || typeof saved.modelId !== "string") return [];
  if (explicit?.length) {
    // An explicit selector authorizes the provider run that began on that model,
    // including subsequent model changes within that same provider.
    for (let i = last; i >= 0; i--) {
      const entry = branch[i]!;
      if (entry.type !== "model_change") continue;
      if (entry.provider !== saved.provider) break;
      if (explicit.some(choice => choice.provider === entry.provider && choice.model === entry.modelId)) {
        return [{ provider: saved.provider, model: saved.modelId }];
      }
    }
    return [];
  }
  if (!launch || !policy || !hasProviderFallbacks(policy) || saved.provider === "llama.cpp" || saved.provider === launch.provider) return [];
  let provider = saved.provider, modelId = saved.modelId;
  for (let i = last - 1; i >= 0; i--) {
    const previous = branch[i]!;
    if (previous.type !== "model_change") continue;
    if (!isProviderId(previous.provider) || typeof previous.modelId !== "string") return [];
    // The owner may change models within a provider. A cross-provider step must
    // still match the configured fallback edge and preserve the model at that step.
    if (previous.provider !== provider) {
      if (previous.modelId !== modelId || !isSubscriptionProvider(previous.provider)
        || !policy.providers[previous.provider]?.includes(provider)) return [];
      provider = previous.provider;
    }
    modelId = previous.modelId;
    if (provider === launch.provider) return [{ provider: saved.provider, model: saved.modelId }];
  }
  // Older/minimal transcripts may not have recorded the initial launch selection.
  return modelId === launch.model && isSubscriptionProvider(launch.provider) && policy.providers[launch.provider]?.includes(provider)
    ? [{ provider: saved.provider, model: saved.modelId }] : [];
}

/** Effective fallback policy for child agents (`fleet` / `subagent`), enabled only when opted in. */
export function childFallbackPolicy(
  policy: FallbackSettings | undefined,
  noFallback?: boolean,
): FallbackSettings | undefined {
  return !noFallback && policy?.children && hasProviderFallbacks(policy) ? policy : undefined;
}
