import { randomUUID } from "node:crypto";
import { CorralCallError, type CorralClient, type CorralReceipt } from "../../tools/corral.ts";
import type { PeerDelivery, PeerInfo, PeerTransport } from "./types.ts";

const ROSTER_TTL_MS = 5_000;
function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function result(receipt: CorralReceipt): Record<string, unknown> {
  if (receipt.state === "rejected") {
    const value = object(receipt.result) ? receipt.result : {};
    throw new CorralCallError(typeof value.code === "string" ? value.code : "peer_refused",
      typeof value.reason === "string" ? value.reason : "Peer operation refused");
  }
  if (receipt.state !== "committed" || !object(receipt.result)) throw new CorralCallError("peer_output", "Invalid peer operation result");
  return receipt.result;
}

/** Cancellation stops waiting, not an already durable send. Never retry a send automatically. */
async function waiting<T>(work: Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return work;
  const { promise, reject } = Promise.withResolvers<never>();
  const cancel = () => reject(signal.reason ?? new DOMException("Aborted", "AbortError"));
  if (signal.aborted) cancel();
  else signal.addEventListener("abort", cancel, { once: true });
  try { return await Promise.race([work, promise]); }
  finally { signal.removeEventListener("abort", cancel); }
}

export function createPeerTransport(client: CorralClient): PeerTransport {
  let self: string | undefined;
  let cached: readonly PeerInfo[] | undefined;
  let expires = 0;
  return {
    self: () => self,
    async roster(signal) {
      signal?.throwIfAborted();
      if (cached && Date.now() < expires) return cached;
      try {
        const value = result(await waiting(client.call("peer_roster", {
          requestId: `rq-${randomUUID().replaceAll("-", "")}`, payload: {},
        }), signal));
        if (!(value.self === undefined || value.self === null || typeof value.self === "string") || !Array.isArray(value.peers)) {
          throw new CorralCallError("peer_output", "Invalid peer roster");
        }
        const peers: PeerInfo[] = value.peers.map((peer: unknown) => {
          if (!object(peer) || typeof peer.key !== "string" || typeof peer.role !== "string"
            || !(peer.title === undefined || typeof peer.title === "string") || !(peer.state === "live" || peer.state === "offline")) {
            throw new CorralCallError("peer_output", "Invalid peer roster entry");
          }
          return { key: peer.key, role: peer.role, ...(typeof peer.title === "string" ? { title: peer.title } : {}),
            state: peer.state === "live" ? "live" : "offline" };
        });
        self = typeof value.self === "string" ? value.self : undefined;
        cached = peers;
        expires = Date.now() + ROSTER_TTL_MS;
        return peers;
      } catch (error) {
        if (error instanceof CorralCallError && error.code === "container_not_found") {
          self = undefined;
          cached = [];
          expires = Date.now() + ROSTER_TTL_MS;
          return cached;
        }
        throw error;
      }
    },
    async send(to, text, signal) {
      signal?.throwIfAborted();
      const value = result(await waiting(client.call("peer_send", {
        requestId: `rq-${randomUUID().replaceAll("-", "")}`, payload: { to, text },
      }), signal));
      if (!(value.state === "delivered" || value.state === "queued" || value.state === "undeliverable")
        || !(value.detail === undefined || typeof value.detail === "string") || !(value.id === undefined || typeof value.id === "string")) {
        throw new CorralCallError("peer_output", "Invalid peer delivery result");
      }
      return { state: value.state, ...(typeof value.detail === "string" ? { detail: value.detail } : {}),
        ...(typeof value.id === "string" ? { id: value.id } : {}) } satisfies PeerDelivery;
    },
  };
}
