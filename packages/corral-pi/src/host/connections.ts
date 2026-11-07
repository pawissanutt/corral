// Versioned Rust -> Pi binding. A model/provider override cannot change its billing route.
import { createHash } from "node:crypto";
import { CONNECTIONS, type ConnectionId, isConnectionId, requirePaidAdmission, resolveConnection } from "../auth/connections.ts";
import type { ProviderId } from "../auth/providers.ts";

export interface ConnectionBinding {
  readonly version: 1;
  readonly connection: ConnectionId;
  readonly identity: string;
  readonly origin: "registered" | "legacy_subscription" | "legacy_bedrock" | "legacy_local";
  readonly candidate: string | null;
  readonly model: string;
  readonly effort: string | null;
  readonly digest: string;
}

export function bindingDigest(binding: Omit<ConnectionBinding, "digest">): string {
  return createHash("sha256").update(JSON.stringify([1, binding.connection, binding.identity, binding.candidate, binding.model, binding.effort, binding.origin])).digest("hex");
}

function origin(connection: ConnectionId, identity: string, candidate: string | null): ConnectionBinding["origin"] | undefined {
  if (identity === connection) return "registered";
  if (candidate === null || isConnectionId(identity)) return undefined;
  if (connection === "anthropic-subscription" || connection === "openai-subscription") return "legacy_subscription";
  if (connection === "amazon-bedrock") return "legacy_bedrock";
  if (connection === "llama.cpp") return "legacy_local";
  return undefined;
}

export function parseBinding(value: unknown, provider: ProviderId, model: string, effort?: string): ConnectionBinding | undefined {
  if (value === undefined || value === null) {
    resolveConnection(provider);
    return undefined;
  }
  const binding = value as ConnectionBinding;
  if (typeof binding !== "object" || Array.isArray(binding) || binding.version !== 1 || !isConnectionId(binding.connection)
      || typeof binding.identity !== "string" || binding.identity.trim() === "" || Buffer.byteLength(binding.identity) > 256
      || /[\u0000-\u001f\u007f-\u009f]/.test(binding.identity)
      || !["registered", "legacy_subscription", "legacy_bedrock", "legacy_local"].includes(binding.origin)
      || binding.origin !== origin(binding.connection, binding.identity, binding.candidate)
      || !(binding.candidate === null || typeof binding.candidate === "string" && binding.candidate.trim() !== ""
        && Buffer.byteLength(binding.candidate) <= 512 && !/[\u0000-\u001f\u007f-\u009f]/.test(binding.candidate))
      || binding.model !== model || binding.effort !== (effort ?? null) || binding.digest !== bindingDigest(binding)) {
    throw new Error("connection_invalid: stale or malformed binding");
  }
  resolveConnection(provider, binding.connection);
  return binding;
}

/** No installed host, including an auxiliary/compaction caller, may activate paid traffic yet. */
export function admitConnection(provider: ProviderId, binding?: ConnectionBinding): ConnectionId {
  const connection = resolveConnection(provider, binding?.connection);
  requirePaidAdmission(connection);
  return connection;
}

export function bindingProvider(binding: ConnectionBinding): ProviderId {
  return CONNECTIONS[binding.connection].provider;
}
