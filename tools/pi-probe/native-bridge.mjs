import { createServer, createConnection } from "node:net";
import { chmodSync } from "node:fs";
import { join } from "node:path";
import { createHash, randomUUID } from "node:crypto";
import { EventEmitter } from "node:events";
import { ProbeFailure, record } from "./credentials.mjs";

/**
 * Disposable control channel, never a Corral production mutation API.
 * @param {{root: string, capability: string, scope: Record<string, string>,
 * generation: () => number,
 * receive: (kind: string, data: Record<string, unknown>) => Promise<Record<string, unknown>>,
 * receipt: (value: Record<string, unknown>) => void}} options
 */
export async function createBridge(options) {
  /** @type {Map<import("node:net").Socket, {generation:number, scope:string}>} */
  const peers = new Map();
  /** @type {Set<import("node:net").Socket>} */
  const sockets = new Set();
  /** @type {Map<string, {digest: string, data: Promise<Record<string, unknown>>}>} */
  const receipts = new Map();
  let sequence = 0;
  const path = join(options.root, "native.sock");
  const server = createServer((socket) => {
    sockets.add(socket);
    socket.on("error", () => socket.destroy());
    socket.on("close", () => { peers.delete(socket); sockets.delete(socket); });
    let buffer = "";
    socket.on("data", (chunk) => {
      buffer += chunk.toString("utf8");
      if (Buffer.byteLength(buffer) > 65_536) { socket.destroy(); return; }
      let boundary;
      while ((boundary = buffer.indexOf("\n")) !== -1) {
        const line = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 1);
        void handle(line).catch(() => socket.destroy());
      }
    });
    /** @param {string} line */
    async function handle(line) {
      /** @type {unknown} */
      const frame = JSON.parse(line);
      if (!record(frame) || typeof frame.id !== "string" || frame.id.length > 80) {
        socket.destroy(); return;
      }
      let data;
      let error;
      try {
        const scope = frame.scope;
        if (frame.capability !== options.capability || !record(scope) ||
            Object.entries(options.scope).some(([key, value]) => scope[key] !== value)) {
          throw new ProbeFailure("foreign_session_scope");
        }
        if (frame.generation !== options.generation()) throw new ProbeFailure("stale_generation");
        if (typeof frame.kind !== "string" || !record(frame.data)) throw new ProbeFailure("invalid_command");
        const digest = createHash("sha256").update(JSON.stringify({
          kind: frame.kind, data: frame.data, generation: frame.generation,
        })).digest("hex");
        const prior = receipts.get(frame.id);
        if (prior && prior.digest !== digest) throw new ProbeFailure("conflicting_request_payload");
        if (receipts.size >= 64 && !prior) throw new ProbeFailure("fixture_receipt_limit");
        peers.set(socket, { generation: options.generation(), scope: JSON.stringify(options.scope) });
        const pending = prior?.data ?? options.receive(frame.kind, frame.data);
        receipts.set(frame.id, { digest, data: pending });
        data = await pending;
      } catch (failure) {
        error = failure instanceof ProbeFailure ? failure.code : "native_bridge_error";
      }
      if (!socket.destroyed) socket.write(JSON.stringify({
        type: "response", id: frame.id, data, error, generation: options.generation(),
      }) + "\n");
    }
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(path, () => { chmodSync(path, 0o600); resolve(undefined); });
  });
  return {
    path,
    /** @param {Record<string, unknown>} value */
    emit(value) {
      const event = { type: "event", sequence: ++sequence, generation: options.generation(), ...value };
      options.receipt(event);
      for (const [socket, identity] of peers) {
        if (identity.generation === options.generation() && identity.scope === JSON.stringify(options.scope)) {
          socket.write(JSON.stringify(event) + "\n");
        }
      }
    },
    async close() {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => server.close(() => resolve(undefined)));
    },
  };
}

/**
 * @param {{path: string, capability: string, scope: Record<string, string>, generation: number}} identity
 */
export async function connectBridge(identity) {
  const socket = createConnection(identity.path);
  const events = new EventEmitter();
  let buffer = "";
  const prefix = randomUUID();
  let count = 0;
  let current = { ...identity, scope: { ...identity.scope } };
  socket.on("data", (chunk) => {
    buffer += chunk.toString("utf8");
    if (Buffer.byteLength(buffer) > 65_536) { socket.destroy(); return; }
    let boundary;
    while ((boundary = buffer.indexOf("\n")) !== -1) {
      const frame = JSON.parse(buffer.slice(0, boundary));
      buffer = buffer.slice(boundary + 1);
      if (frame.type === "response") events.emit(frame.id, frame);
      else events.emit(frame.event, frame);
    }
  });
  await new Promise((resolve, reject) => {
    socket.once("error", reject);
    socket.once("connect", () => resolve(undefined));
  });
  socket.on("error", () => socket.destroy());
  /** @param {string} name */
  function waitFor(name) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { events.removeListener(name, ready); reject(new ProbeFailure("native_event_deadline")); }, 45_000);
      /** @param {Record<string, unknown>} value */
      function ready(value) { clearTimeout(timer); resolve(value); }
      events.once(name, ready);
    });
  }
  return {
    waitFor,
    /** @param {string} kind @param {Record<string, unknown>} [data] @param {Record<string, unknown>} [overrides] */
    async request(kind, data = {}, overrides = {}) {
      const id = typeof overrides.id === "string" ? overrides.id : prefix + ":" + ++count;
      const response = waitFor(id);
      socket.write(JSON.stringify({ id, kind, data, ...current, ...overrides }) + "\n");
      return response;
    },
    /** @param {string} kind @param {Record<string, unknown>} data @param {string} id */
    sendAndClose(kind, data, id) {
      socket.end(JSON.stringify({ id, kind, data, ...current }) + "\n");
    },
    /** @param {number} generation @param {string} sessionId */
    setSession(generation, sessionId) {
      current = { ...current, generation, scope: { ...current.scope, native_session_id: sessionId } };
    },
    close() { socket.destroy(); },
  };
}
