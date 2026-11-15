import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import * as sdk from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import { bindingDigest } from "../src/host/connections.ts";
import type { ConnectionBinding } from "../src/host/connections.ts";
import { CONTINUE_ASSISTANT_EXIT, HostContinuation, parseNotice, readContext } from "../src/host/continuation.ts";
import type { ContinuationNotice } from "../src/host/continuation.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import type { HostSessionOptions } from "../src/host/session.ts";
import type { CorralOp, HostIdentity, RoleContext } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import type { CorralCall, CorralReceipt } from "../src/tools/corral.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import fixtureAuth from "./fixtures/auth/index.ts";

const identity: HostIdentity = { store: "st-0123456789abcdef", project: "demo", role: "assistant", assistant: "A-1", generation: 2 };
const target = { version: 1 as const, connection: "openai-api" as const, identity: "openai-api", origin: "registered" as const,
  candidate: "paid", model: "gpt-6.1-sol", effort: null };
const binding: ConnectionBinding = { ...target, digest: bindingDigest(target) };
const notice: ContinuationNotice = { version: 1, generation: 2, binding_digest: binding.digest,
  source_connection: "openai-subscription", target_connection: "openai-api", candidate: "paid", reason: "subscription quota",
  episode: "episode-fixture", allowance_nanos: 1_000_000, consumed_or_reserved_nanos: 10,
  context: { kind: "resumed-session" } };

class HelloClient extends CorralClient {
  continuation: unknown;
  endGate: (() => Promise<void>) | undefined;
  override async call(op: CorralOp, call: CorralCall): Promise<CorralReceipt> {
    if (op === "event" && call.payload.kind === "turn_ended") await this.endGate?.();
    const receipt = await super.call(op, call);
    if (op !== "hello") return receipt;
    return { ...receipt, result: { ...(typeof receipt.result === "object" ? receipt.result : {}), continuation: this.continuation } };
  }
}

test("notice parsing refuses stale authority and malformed native fields", () => {
  expect(parseNotice(undefined, identity, binding)).toBeUndefined();
  expect(parseNotice(null, identity, binding)).toBeUndefined();
  expect(parseNotice(notice, identity, binding)).toEqual(notice);
  const invalid = [
    { ...notice, generation: 1 }, { ...notice, binding_digest: "0".repeat(64) },
    { ...notice, context: { kind: "unknown" } }, { ...notice, context: { kind: "resumed-session", path: "/unexpected" } },
    { ...notice, allowance_nanos: Number.MAX_SAFE_INTEGER + 1 }, { ...notice, consumed_or_reserved_nanos: -1 },
    { ...notice, candidate: "different" }, { ...notice, source_connection: "anthropic-subscription" },
    { ...notice, reason: "" }, { ...notice, episode: 1 }, { ...notice, extra: true },
    { ...notice, context: { kind: "checkpoint", digest: "bad", path: "/context", bytes: 1 } },
    { ...notice, context: { kind: "checkpoint", digest: "a".repeat(64), path: "relative", bytes: 1 } },
    { ...notice, context: { kind: "checkpoint", digest: "a".repeat(64), path: "/context", bytes: 256 * 1024 + 1 } },
    { ...notice, context: { kind: "fresh-read-only", reason: "missing draft" } },
  ];
  for (const value of invalid) expect(() => parseNotice(value, identity, binding)).toThrow("continuation_invalid");
  expect(() => parseNotice(notice, identity, undefined)).toThrow("binding digest mismatch");
  for (const role of ["reviewer", "planner", "plan_reviewer"] as const) {
    expect(parseNotice({ ...notice, context: { kind: "fresh-read-only", reason: "no committed draft" } },
      { ...identity, role }, binding)?.context.kind).toBe("fresh-read-only");
  }
});

test("context reader verifies artifact bytes rather than a manifest digest", () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-continuation-context-"));
  const path = join(dir, "context");
  const text = "verified neutral context π";
  const checkpoint: ContinuationNotice = { ...notice, context: { kind: "checkpoint", path,
    bytes: Buffer.byteLength(text), digest: sha256Hex(text) } };
  try {
    writeFileSync(path, text);
    expect(readContext(checkpoint)).toBe(text);
    writeFileSync(path, "x".repeat(Buffer.byteLength(text)));
    expect(() => readContext(checkpoint)).toThrow("SHA-256 mismatch");
    writeFileSync(path, "short");
    expect(() => readContext(checkpoint)).toThrow("size mismatch");
    rmSync(path);
    expect(() => readContext(checkpoint)).toThrow("unavailable");
    expect(readContext(notice)).toBeUndefined();
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("committed hello refuses invalid paid context before binding and consumes its switch notice once", async () => {
  const assets = buildFixtureInstall();
  const dir = mkdtempSync(join(tmpdir(), "corral-continuation-hello-"));
  let host: HostSession | undefined;
  const oldWrite = process.stderr.write;
  const shown: string[] = [];
  try {
    const modules = await loadModules(assets.dir, "worker");
    const cwd = join(dir, "work"); mkdirSync(cwd);
    const fake = writeFakeCorral(dir);
    const events = new EventLog(identity.generation);
    const manager = sdk.SessionManager.create(cwd, join(dir, "sessions"));
    const client = new HelloClient({ corral: fake.path, root: dir, identity, hostCap: "h".repeat(64),
      sessionId: () => host?.sessionId() ?? "" });
    const options: HostSessionOptions & { binding?: ConnectionBinding } = { sdk, identity, cwd, agentDir: join(dir, "agent"),
      model: "gpt-6.1-sol", modules, client, events, sessionManager: manager,
      network: guardedFetch(fixtureAuth.transport!({ provider: "openai-codex", apiEndpoint: "", refreshEndpoint: "" })) };
    host = await HostSession.create(options);
    // Isolate the committed-hello authority gate; catalog admission is covered cross-lane.
    Object.assign(options, { binding });
    const contextPath = join(dir, "context");
    const manifest = JSON.stringify({ owner_input_ids: ["A-1/g1/i1"], receipts: [] });
    const checkpoint: ContinuationNotice = { ...notice, context: { kind: "checkpoint", path: contextPath,
      bytes: Buffer.byteLength(manifest), digest: sha256Hex(manifest) } };
    const hello = () => host!.hello("n".repeat(64), null, "c".repeat(64));
    for (const invalid of [undefined, { ...notice, generation: 1 }, { ...notice, binding_digest: "0".repeat(64) }, checkpoint]) {
      client.continuation = invalid;
      await expect(hello()).rejects.toThrow(invalid === undefined ? "paid_admission_required" : "continuation_");
      expect(host.bound).toBeFalse();
    }
    expect(fake.calls().some(call => call.envelope.payload.kind === "turn_started")).toBeFalse();
    writeFileSync(contextPath, manifest);
    client.continuation = checkpoint;
    process.stderr.write = ((chunk: string | Uint8Array) => { shown.push(String(chunk)); return true; }) as typeof process.stderr.write;
    await hello();
    host.markAttached("headless");
    expect(host.bound).toBeTrue();
    expect(shown).toHaveLength(1);
    expect(events.since(0, 64 * 1024).events.filter(event => event.kind === "continuation_notice")).toHaveLength(1);
    // A replacement host consuming the same generation's committed hello shares the Pi marker.
    await host.close(); host = undefined;
    // The constructor captures its options object, so restore the API binding only after create.
    const nextOptions = { ...options, binding: undefined as ConnectionBinding | undefined };
    host = await HostSession.create(nextOptions);
    nextOptions.binding = binding;
    await hello();
    host.markAttached("headless");
    expect(shown).toHaveLength(1);
    expect(events.since(0, 64 * 1024).events.filter(event => event.kind === "continuation_notice")).toHaveLength(1);
  } finally {
    process.stderr.write = oldWrite;
    await host?.close(); assets.cleanup(); rmSync(dir, { recursive: true, force: true });
  }
});

test("PM and assistant receive the verified manifest before fresh conversation messages", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-continuation-first-"));
  const path = join(dir, "context");
  const manifest = JSON.stringify({ receipts: [{ request_id: "original" }] });
  try {
    writeFileSync(path, manifest);
    for (const role of ["pm", "assistant"] as const) {
      const current = { ...identity, role };
      const continuation = new HostContinuation({ identity: current, binding: () => binding,
        manager: sdk.SessionManager.create(dir, join(dir, role)), events: new EventLog(2), notify: () => {} });
      continuation.accept({ ...notice, context: { kind: "checkpoint", path, digest: sha256Hex(manifest), bytes: Buffer.byteLength(manifest) } });
      let inject: ((event: { messages: { role: "user"; timestamp: number; content: string }[] }) =>
        { messages: { role: string; content: string }[] } | undefined) | undefined;
      const pi = { on: (name: string, handler: unknown) => {
        if (name === "context_with_system") inject = handler as typeof inject;
      } } as unknown as ExtensionAPI;
      const context: RoleContext = { identity: current, cwd: dir, model: "gpt-6.1-sol", continuation: continuation.notice };
      continuation.extend(pi, context);
      const result = inject?.({ messages: [{ role: "user", timestamp: 1, content: "fresh task" }] });
      expect(result?.messages).toHaveLength(2);
      expect(result?.messages[0]?.content).toContain(manifest);
      expect(result?.messages[1]?.content).toBe("fresh task");
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("only a drained idle assistant requests exit 78 after a committed subscription quota failure", async () => {
  const assets = buildFixtureInstall();
  const dir = mkdtempSync(join(tmpdir(), "corral-continuation-exit-"));
  const oldFetch = globalThis.fetch;
  let host: HostSession | undefined;
  try {
    const modules = await loadModules(assets.dir, "worker");
    const cwd = join(dir, "work"); mkdirSync(cwd);
    for (const variant of ["quota", "queued", "other", "pm"] as const) {
      const current: HostIdentity = { ...identity, role: variant === "pm" ? "pm" : "assistant" };
      const fakeDir = join(dir, variant); mkdirSync(fakeDir);
      const fake = writeFakeCorral(fakeDir);
      const events = new EventLog(2);
      const exits: number[] = [];
      const held = Promise.withResolvers<void>();
      const release = Promise.withResolvers<void>();
      const ended = Promise.withResolvers<void>();
      const commit = Promise.withResolvers<void>();
      const network = guardedFetch(async () => {
        held.resolve(); await release.promise;
        return Response.json({ error: { type: variant === "other" ? "invalid_request_error" : "usage_limit_reached" } },
          { status: variant === "other" ? 400 : 429 });
      });
      globalThis.fetch = network.fetch as typeof fetch;
      const client = new HelloClient({ corral: fake.path, root: fakeDir, identity: current, hostCap: "h".repeat(64),
        sessionId: () => host?.sessionId() ?? "" });
      client.endGate = async () => { ended.resolve(); await commit.promise; };
      host = await HostSession.create({ sdk, identity: current, cwd, agentDir: join(fakeDir, "agent"), model: "gpt-6.1-sol",
        modules, client, events, network, sessionManager: sdk.SessionManager.create(cwd, join(fakeDir, "sessions")),
        continueAssistant: () => { exits.push(CONTINUE_ASSISTANT_EXIT); } });
      await host.hello("n".repeat(64), null, "c".repeat(64));
      const turn = host.session.prompt("offline quota fixture");
      await held.promise;
      expect(host.currentTurn()).toBe(1);
      expect(exits).toEqual([]);
      release.resolve(); await turn; await ended.promise;
      expect(exits).toEqual([]); // The provider/turn-end report chain is not drained yet.
      if (variant === "queued") await host.session.followUp("pending owner input");
      commit.resolve();
      await host.drained(); await host.continued();
      expect(exits).toEqual(variant === "quota" ? [78] : []);
      host.session.clearQueue();
      await host.close(); host = undefined;
    }
  } finally {
    await host?.close(); globalThis.fetch = oldFetch;
    assets.cleanup(); rmSync(dir, { recursive: true, force: true });
  }
});
