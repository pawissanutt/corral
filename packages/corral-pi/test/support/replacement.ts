// Every in-process session replacement Pi offers (/new, /resume, /fork before and at an entry,
// import), attempted against a bound host. Corral observes and recovers only the file hello bound,
// so each must be refused before it creates, copies or switches to another session.
import { expect } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { BridgeEvent, EventLog } from "../../src/bridge/events.ts";
import type { HostSession } from "../../src/host/session.ts";
import { connect } from "./bridge-client.ts";
import { within } from "./wait.ts";
import { ok } from "./workspace.ts";

/** Retries `attempt` until it yields a value; the loop stops at the deadline instead of outliving it. */
async function until<T>(what: string, ms: number, attempt: () => Promise<T | undefined> | T | undefined): Promise<T> {
  let stopped = false;
  const loop = (async () => {
    while (!stopped) {
      const found = await attempt();
      if (found !== undefined) return found;
      await new Promise((resolve) => setImmediate(resolve));
    }
    throw new Error(`stopped waiting for ${what}`);
  })();
  try {
    return await within(loop, ms, what);
  } finally {
    stopped = true;
    loop.catch(() => undefined);
  }
}

/**
 * The owner types `/new` into a live host's pane; resolves with the host's refusal event once the
 * bridge reports it, and checks the pane shows the refusal.
 */
export async function ownerNewIsRefused(pane: string, generationDir: string, generation: number): Promise<BridgeEvent> {
  const cap = readFileSync(join(generationDir, "ctl.cap"), "utf8");
  const client = await connect(join(generationDir, "host.sock"));
  let id = 0;
  const request = async (command: string, args: Record<string, unknown>) =>
    (await client.request({ version: 1, id: `owner-new-${id++}`, ctl_cap: cap, generation, command, args })).result ?? {};
  try {
    await until("the pane to run InteractiveMode", 60_000,
      async () => (await request("get_state", {})).interactive === true ? true : undefined);
    let seq = (await request("events_since", { seq: Number.MAX_SAFE_INTEGER })).last as number;
    ok(["herdr", "--session", "corral-test", "pane", "send-text", pane, "/new"]);
    ok(["herdr", "--session", "corral-test", "pane", "send-keys", pane, "enter"]);
    const refused = await until("the host to refuse the owner's /new", 60_000, async () => {
      const page = await request("events_since", { seq }) as { events: BridgeEvent[]; next: number };
      seq = page.next;
      return page.events.find((event) => event.kind === "session_replacement_refused");
    });
    // The notice renders after the event, so the pane is read until it shows.
    await until("the pane to show the refusal", 60_000, () =>
      ok(["herdr", "--session", "corral-test", "pane", "read", pane]).replace(/\s+/g, " ")
        .includes("corral: a new session is refused") ? true : undefined);
    return refused;
  } finally {
    client.close();
  }
}

export async function expectReplacementsRefused(host: HostSession, events: EventLog): Promise<void> {
  expect(host.bound).toBeTrue();
  const boundId = host.sessionId();
  const boundFile = host.session.sessionFile;
  if (!boundFile || !existsSync(boundFile)) throw new Error("the bound session has no file on disk yet");
  const sessionDir = host.session.sessionManager.getSessionDir();
  const files = readdirSync(sessionDir).sort();
  const created = (host.state().session as { created: number }).created;
  const user = host.session.sessionManager.getEntries()
    .find((entry) => entry.type === "message" && entry.message.role === "user");
  const leaf = host.session.sessionManager.getLeafId();
  if (!user || !leaf) throw new Error("the bound session has no user message to fork from");
  const from = events.last;
  const scratch = mkdtempSync(join(tmpdir(), "corral-pi-replace-"));
  try {
    const foreign = join(scratch, "foreign.jsonl");
    writeFileSync(foreign, `${JSON.stringify({ type: "session", version: 3, id: "foreign", timestamp: new Date(0).toISOString(),
      cwd: host.session.sessionManager.getCwd() })}\n`);
    const attempts: [string, () => Promise<{ cancelled: boolean }>][] = [
      ["newSession", () => host.runtime.newSession()],
      ["switchSession", () => host.runtime.switchSession(foreign)],
      ["fork before", () => host.runtime.fork(user.id)],
      ["fork at", () => host.runtime.fork(leaf, { position: "at" })],
      ["importFromJsonl", () => host.runtime.importFromJsonl(foreign)],
    ];
    for (const [name, attempt] of attempts) {
      expect([name, (await attempt()).cancelled]).toEqual([name, true]);
      expect([name, host.session.sessionFile]).toEqual([name, boundFile]);
    }
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
  expect([host.session.sessionId, host.session.sessionFile, host.sessionId()]).toEqual([boundId, boundFile, boundId]);
  expect((host.state().session as { created: number }).created).toBe(created);
  expect(readdirSync(sessionDir).sort()).toEqual(files);
  const refused = events.since(from, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events
    .filter((event) => event.kind === "session_replacement_refused");
  expect(refused.map((event) => event.operation)).toEqual([
    "a new session", "switching or importing a session", "forking the session", "forking the session",
    "switching or importing a session",
  ]);
  for (const event of refused) {
    expect(event.session_id).toBe(boundId);
    expect(String(event.message)).toContain(`refused while this pane is bound to corral session ${boundId}`);
  }
}
