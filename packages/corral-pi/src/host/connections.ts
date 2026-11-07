// Versioned Rust -> Pi binding. A model/provider override cannot change its billing route.
import { createHash } from "node:crypto";
import { CONNECTIONS, type ConnectionId, isConnectionId, requirePaidAdmission, resolveConnection } from "../auth/connections.ts";
import type { ProviderId } from "../auth/providers.ts";

export interface ConnectionBinding {
  readonly version: 1;
  readonly connection: ConnectionId;
  readonly candidate: string | null;
  readonly model: string;
  readonly effort: string | null;
  readonly digest: string;
}

export function bindingDigest(binding: Omit<ConnectionBinding, "digest">): string {
  return createHash("sha256").update(JSON.stringify([1, binding.connection, binding.candidate, binding.model, binding.effort])).digest("hex");
}

export function parseBinding(value: unknown, provider: ProviderId, model: string, effort?: string): ConnectionBinding | undefined {
  if (value === undefined || value === null) {
    resolveConnection(provider);
    return undefined;
  }
  const binding = value as ConnectionBinding;
  if (typeof binding !== "object" || Array.isArray(binding) || binding.version !== 1 || !isConnectionId(binding.connection)
      || !(binding.candidate === null || typeof binding.candidate === "string" && /^[A-Za-z0-9._:+-]{1,128}$/.test(binding.candidate))
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
