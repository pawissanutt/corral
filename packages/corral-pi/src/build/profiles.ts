import { isProviderId } from "../auth/providers.ts";
import { isApiConnection } from "../auth/connections.ts";
import { parseBinding } from "../host/connections.ts";
import type { BuildContext, BuildProfileRoute } from "./types.ts";

/** Used for inherited launch messages and saved route snapshots, never as financial admission. */
export function parseBuildProfileRoute(value: unknown): BuildProfileRoute {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("build_profile_invalid: route must be an object");
  const route = value as BuildProfileRoute;
  if (typeof route.name !== "string" || !route.name.trim() || route.name.length > 128 || typeof route.label !== "string"
      || !isProviderId(route.provider) || typeof route.model !== "string" || !route.model.trim() || /\s/.test(route.model)
      || (route.effort !== undefined && typeof route.effort !== "string")) throw new Error("build_profile_invalid: malformed route");
  const binding = parseBinding(route.binding, route.provider, route.model, route.effort);
  const paid = Boolean(binding && isApiConnection(binding.connection));
  if (paid !== Boolean(route.paid)) throw new Error("build_profile_invalid: API route lacks approved provenance");
  if (route.paid && (!/^[a-f0-9]{64}$/i.test(route.paid.policy_digest) || typeof route.paid.approval !== "string" || !route.paid.approval.trim()
      || !Number.isSafeInteger(route.paid.allowance_nanos) || route.paid.allowance_nanos <= 0 || !Array.isArray(route.paid.candidates)
      || route.paid.candidates.length !== 1 || route.paid.candidates[0]?.digest !== binding?.digest)) throw new Error("build_profile_invalid: malformed approved provenance");
  return { ...route, ...(binding ? { binding } : {}) };
}

/** Preview only. Runtime/auth/model preflight and a supervised restart precede persistence. */
export async function selectBuildProfile(build: BuildContext, name: string): Promise<BuildProfileRoute> {
  const selected = build.profileCatalog?.find(route => route.name === name);
  if (!selected || selected.available === false) throw new Error(selected?.reason ?? `profile_unavailable: ${name}`);
  const route = parseBuildProfileRoute(selected);
  const current = build.session.route;
  if (current?.paid && (route.name !== current.name || route.binding?.digest !== current.binding?.digest
      || JSON.stringify(route.paid) !== JSON.stringify(current.paid))) {
    throw new Error("paid_admission_required: paid Build route is sticky; profile changes cannot refresh its allowance");
  }
  if (build.session.bare && route.paid) throw new Error("paid_admission_required: Bare API profiles are unavailable; use --build --profile");
  return route;
}
