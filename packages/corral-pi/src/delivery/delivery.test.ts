// The real delivery module inside a real HostSession for each of host-bridge's fixture roles
// (worker, reviewer, PM): bridge submit_input writes exactly one corral_delivery entry per key
// into the bound Pi session file, carrying the text's SHA-256, and a replayed key writes none.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../bridge/events.ts";
import { startBridge } from "../bridge/server.ts";
import { loadModules } from "../host/modules.ts";
import { loadPeerLookup } from "../host/peer.ts";
import { guardedFetch, restrictEnvironment } from "../host/restrict.ts";
import { HostSession, authContext } from "../host/session.ts";
import type { Role } from "../roles/types.ts";
import { CorralClient, sha256Hex } from "../tools/corral.ts";
import { connect } from "../../test/support/bridge-client.ts";
import { writeFakeCorral } from "../../test/support/fake-corral.ts";
import { type FixtureInstall, buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { nextEvent } from "../../test/support/wait.ts";
import { ENTRY_TYPE } from "./index.ts";

const CTL = "d".repeat(64);
const originalFetch = globalThis.fetch;
const RECIPIENTS: Record<Exclude<Role, "planner" | "plan_reviewer" | "assistant">, { key: string; identity: Record<string, unknown> }> = {
  worker: { key: "T-001_R1", identity: { task: "T-001", run: "R1" } },
  reviewer: { key: "T-001_R1_rv-R1-review1", identity: { task: "T-001", run: "R1", review: "R1-review1", callback: "g1:c1" } },
  pm: { key: "pm", identity: {} },
};

let assets: FixtureInstall;
let work: string;

beforeAll(() => {
  // The package's own src/delivery/index.ts stays: only roles and auth are fixtures.
  assets = buildFixtureInstall({ omit: ["delivery"] });
  work = mkdtempSync(join(tmpdir(), "corral-pi-delivery-"));
  restrictEnvironment(process.env);
});

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

function entries(file: string): { customType?: string; data: Record<string, unknown> }[] {
  return readFileSync(file, "utf8").trim().split("\n").map((line) => JSON.parse(line))
    .filter((entry) => entry.customType === ENTRY_TYPE);
}

for (const role of ["worker", "reviewer", "pm"] as const) {
  test(`${role}: one corral_delivery entry per inbox key, with the envelope SHA-256, before Pi sees the text`, async () => {
    const modules = await loadModules(assets.dir, role);
    expect(modules.role.role).toBe(role);
    expect((modules.delivery.state?.() as { module?: string }).module).toBe(ENTRY_TYPE);
    const dir = join(work, role);
    mkdirSync(join(dir, "worktree"), { recursive: true });
    const identity = {
      store: "st-0123456789abcdef", project: "demo", role, generation: 1, ...RECIPIENTS[role].identity,
    } as ConstructorParameters<typeof CorralClient>[0]["identity"];
    const events = new EventLog(1);
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("the fixture auth module supplies a transport");
    const network = guardedFetch(transport, () => undefined);
    globalThis.fetch = network.fetch as typeof globalThis.fetch;
    const sdk = await import("@earendil-works/pi-coding-agent");
    const corral = writeFakeCorral(dir);
    let host: HostSession | undefined;
    const client = new CorralClient({ corral: corral.path, root: dir, identity, hostCap: "h".repeat(64), sessionId: () => host!.sessionId() });
    host = await HostSession.create({
      sdk, identity, cwd: join(dir, "worktree"), agentDir: join(dir, "pi-agent"), model: "gpt-6.1-sol", modules, client, events,
      network, sessionManager: sdk.SessionManager.create(join(dir, "worktree"), join(dir, "sessions")),
    });
    await host.hello("n".repeat(64), `pane-${role}`, sha256Hex(CTL));
    const server = await startBridge({
      path: join(dir, "host.sock"), ctlCap: CTL, generation: 1, uid: process.getuid?.() ?? -1, peer: await loadPeerLookup(),
      events, handlers: {
        hello: () => ({ bound: true }), state: () => host!.state(), cancel: async () => ({}), shutdown: () => undefined,
        submit: (input, requestId) => host!.submit(input, requestId),
      },
    });
    const bridge = await connect(join(dir, "host.sock"));
    const key = `${RECIPIENTS[role].key}:env1`;
    const text = `[corral envelope ${RECIPIENTS[role].key}@env1]\n\n[corral ${RECIPIENTS[role].key}#1 owner_note]\nReply only DELIVERED.`;
    const args = { text, busy_policy: "follow_up", idempotency_key: key, origin: "corral-inbox" };
    const frame = (id: string) => ({ version: 1, id, ctl_cap: CTL, generation: 1, command: "submit_input", args });
    const readback = () => modules.delivery.state?.() as { accepted: number; recent: { idempotency_key: string }[] };
    const before = readback().accepted;
    const ended = nextEvent(events, (event) => event.kind === "turn_ended");
    const first = (await bridge.request(frame("rq-first"))).result;
    expect(first?.disposition).toBe("started");
    await ended;
    await host.drained();
    const replay = (await bridge.request(frame("rq-replay"))).result;
    expect([replay?.disposition, replay?.replayed]).toEqual(["started", true]);

    const file = (host.state().session as { file: string }).file;
    const written = entries(file);
    expect(written).toHaveLength(1);
    expect(written[0]?.data).toEqual({
      version: 1, idempotency_key: key, request_id: "rq-first", origin: "corral-inbox", busy_policy: "follow_up",
      text_sha256: sha256Hex(text), text_bytes: Buffer.byteLength(text), generation: 1, session_id: host.sessionId(),
    });
    // The entry precedes the user message Pi recorded for the same text.
    const lines = readFileSync(file, "utf8").trim().split("\n").map((line) => JSON.parse(line));
    const entryAt = lines.findIndex((entry) => entry.customType === ENTRY_TYPE);
    const userAt = lines.findIndex((entry) => entry.type === "message" && entry.message?.role === "user");
    expect(entryAt >= 0 && entryAt < userAt).toBeTrue();
    expect([readback().accepted - before, readback().recent.at(-1)?.idempotency_key]).toEqual([1, key]);
    expect(readback().recent.length <= 8).toBeTrue();
    bridge.close();
    await server.close();
  }, 60_000);
}
