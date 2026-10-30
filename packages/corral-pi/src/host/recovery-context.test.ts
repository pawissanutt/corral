// The recovery notice reaches the model without entering Pi's session file (T-252 recovery):
// a real HostSession with the production delivery module and the host's recovery context
// admits a notice-first envelope, the model's request carries the notice, and the session file
// keeps the corral_delivery entry over the text corral sent but only a placeholder for the notice.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../bridge/events.ts";
import { ENTRY_TYPE } from "../delivery/index.ts";
import { CorralClient, sha256Hex } from "../tools/corral.ts";
import { writeFakeCorral } from "../../test/support/fake-corral.ts";
import { type FixtureInstall, buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { nextEvent } from "../../test/support/wait.ts";
import { holdContextOnly, withRecoveryContext } from "./lifecycle.ts";
import { loadModules } from "./modules.ts";
import { guardedFetch, restrictEnvironment } from "./restrict.ts";
import { HostSession, authContext } from "./session.ts";

const originalFetch = globalThis.fetch;
const KEY = "T-001_R1";
const NOTICE = `[corral ${KEY}#4 recovery_notice]\ncorral resumed this host as generation 2 after generation 1 `
  + "was lost. Calls left without a result:\n- edit (call c-edit): outcome_unknown - CLASSIFIED-7F3A\n"
  + "Reply only NOTICE_SEEN.";
const QUEUED = `[corral ${KEY}#5 owner_reply]\nAlso handle the empty input.`;
const ENVELOPE = `[corral envelope ${KEY}@env3]\n\n${NOTICE}\n\n${QUEUED}`;

let assets: FixtureInstall;
let work: string;

beforeAll(() => {
  assets = buildFixtureInstall({ omit: ["delivery"] });
  work = mkdtempSync(join(tmpdir(), "corral-pi-recovery-context-"));
  restrictEnvironment(process.env);
});

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("an envelope's recovery members are held as placeholders and every other member is kept", () => {
  const split = holdContextOnly(ENVELOPE);
  expect(split?.held.map((member) => member.text)).toEqual([NOTICE]);
  const placeholder = split?.held[0]?.placeholder ?? "";
  expect(placeholder.startsWith(`[corral ${KEY}#4 recovery_notice: given to this host as context only`)).toBeTrue();
  expect(split?.text).toBe(`[corral envelope ${KEY}@env3]\n\n${placeholder}\n\n${QUEUED}`);
  expect(holdContextOnly(`[corral envelope ${KEY}@env4]\n\n${QUEUED}`)).toBeUndefined();
  expect(holdContextOnly("Reply only PLAIN.")).toBeUndefined();
});

test("a resumed host's model reads the recovery notice, and its session file keeps only a placeholder", async () => {
  // Given a generation-2 worker host with the production delivery module and recovery context:
  const modules = withRecoveryContext(await loadModules(assets.dir, "worker"));
  const dir = join(work, "worker");
  mkdirSync(join(dir, "worktree"), { recursive: true });
  const identity = {
    store: "st-0123456789abcdef", project: "demo", role: "worker", generation: 2, task: "T-001", run: "R1",
  } as ConstructorParameters<typeof CorralClient>[0]["identity"];
  const events = new EventLog(2);
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("the fixture auth module supplies a transport");
  globalThis.fetch = guardedFetch(transport, () => undefined).fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const corral = writeFakeCorral(dir);
  let host: HostSession | undefined;
  const client = new CorralClient({ corral: corral.path, root: dir, identity, hostCap: "h".repeat(64), sessionId: () => host!.sessionId() });
  host = await HostSession.create({
    sdk, identity, cwd: join(dir, "worktree"), agentDir: join(dir, "pi-agent"), model: "gpt-6.1-sol", modules, client, events,
    network: guardedFetch(transport, () => undefined),
    sessionManager: sdk.SessionManager.create(join(dir, "worktree"), join(dir, "sessions")),
  });
  await host.hello("n".repeat(64), "pane-worker", sha256Hex("c".repeat(64)));

  // When corral delivers the notice-first envelope and the turn it starts ends:
  const ended = nextEvent(events, (event) => event.kind === "turn_ended");
  const input = { text: ENVELOPE, busyPolicy: "follow_up" as const, idempotencyKey: `${KEY}:env3`, origin: "corral-inbox" };
  expect((await host.submit(input, "rq-recovery")).disposition).toBe("started");
  await ended;
  await host.drained();

  // Then the model answered from the notice, the delivery entry covers the text corral sent, and
  // the session file holds the placeholder and the queued reply but not the notice's text:
  const file = (host.state().session as { file: string }).file;
  const raw = readFileSync(file, "utf8");
  const lines = raw.trim().split("\n").map((line) => JSON.parse(line));
  const delivery = lines.filter((entry) => entry.customType === ENTRY_TYPE);
  expect(delivery.map((entry) => entry.data.text_sha256)).toEqual([sha256Hex(ENVELOPE)]);
  const text = (entry: { message?: { content?: unknown } }) => JSON.stringify(entry.message?.content ?? "");
  const users = lines.filter((entry) => entry.type === "message" && entry.message?.role === "user").map(text);
  expect(users).toHaveLength(1);
  expect(users[0]).toContain(`[corral ${KEY}#4 recovery_notice: given to this host as context only`);
  expect(users[0]).toContain("Also handle the empty input.");
  const replies = lines.filter((entry) => entry.type === "message" && entry.message?.role === "assistant").map(text);
  expect(replies.some((reply) => reply.includes("NOTICE_SEEN"))).toBeTrue();
  expect(raw.includes("CLASSIFIED-7F3A")).toBeFalse();
  expect(raw.includes("Reply only NOTICE_SEEN")).toBeFalse();
}, 60_000);
