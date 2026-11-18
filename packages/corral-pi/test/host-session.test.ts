import { afterAll, beforeAll, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Text, stripTerminalSequences, visibleWidth, type Component } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../node_modules/@earendil-works/pi-tui/dist/layout.js";

import { EventLog } from "../src/bridge/events.ts";
import { startBridge } from "../src/bridge/server.ts";
import { connect, testHandlers } from "./support/bridge-client.ts";
import type { SessionInput } from "../src/bridge/protocol.ts";
import { loadModules } from "../src/host/modules.ts";
import { BUILTIN_EXTENSIONS, NetworkRefused, guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { forbiddenRuns, holdNextInput } from "./fixtures/roles/fixture-role.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";
import { type FixtureInstall, buildFixtureInstall } from "./support/fixture-install.ts";
import { expectReplacementsRefused } from "./support/replacement.ts";
import { nextEvent, within } from "./support/wait.ts";
import { historyUI } from "./support/history-ui.ts";

const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1" };
const originalFetch = globalThis.fetch;

let assets: FixtureInstall;
let work: string;
let events: EventLog;
let host: HostSession;

const submit = (text: string, busyPolicy: SessionInput["busyPolicy"]) =>
  host.submitLocal({ text, busyPolicy, origin: "owner" });

beforeAll(async () => {
  assets = buildFixtureInstall();
  work = mkdtempSync(join(tmpdir(), "corral-pi-host-"));
  const cwd = join(work, "worktree");
  mkdirSync(join(cwd, ".pi/skills/leak"), { recursive: true });
  mkdirSync(join(cwd, ".pi/prompts"), { recursive: true });
  writeFileSync(join(cwd, "AGENTS.md"), "Inherited context that must not load.\n");
  writeFileSync(join(cwd, ".pi/skills/leak/SKILL.md"), "---\nname: leak\ndescription: must not load\n---\nLeak.\n");
  writeFileSync(join(cwd, ".pi/prompts/leak.md"), "Leaked prompt template.\n");
  restrictEnvironment(process.env);
  const modules = await loadModules(assets.dir, "worker");
  events = new EventLog();
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("the fixture auth module supplies a transport");
  const network = guardedFetch(transport, (destination) => events.emit("network_refused", { destination }));
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const client = new CorralClient({ corral: writeFakeEngine(work).path, root: work, cwd, identity });
  client.history = async request => {
    if (request.kind === "tasks") return [{ id: "T-999", title: "Historical task" }];
    if (request.kind === "task") return { id: "T-999", runs: [{ id: "R1", ended: "yesterday", summary: "Recorded historical summary" }] };
    if (request.kind === "transcript") return '{"type":"message","message":{"role":"user","content":"Historical input"}}\n';
    throw new Error("Unsupported history fixture read");
  };
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "pi-agent"), model: "gpt-6.1-sol", modules, client, events, network,
    sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
  await host.session.bindExtensions({});
}, 60_000);

afterAll(async () => {
  await host?.close();
  await host?.drained();
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

let main: Component | undefined;
/**
 * What the session shows around the conversation without a terminal or model call: header rows and
 * tabs (main column), the wide rail, and the collapsed sidebar row used below the rail width.
 */
function captureInspector(bound: boolean): void {
  const panels = host.panels;
  if (!panels) throw new Error("rich host supplies native panels");
  main ??= panels.mainView(new Text("", 0, 0));
  const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
  const chrome = plain(renderLayoutFrame(main, 105, 30, () => {}).lines.slice(0, 3));
  const rail = plain(panels.renderSidebar(34, 30));
  const inline = panels.sidebar.row(panels.workbench.sideSource(), 80, false).lines;
  expect(host.session.model?.id).toBe("gpt-6.1-sol");
  // Header row 1 names the bound task and run; the tab row offers the task-bound views.
  expect(chrome[0]).toContain("T-001 · R1");
  for (const tab of ["Task", "Chat", "Agents", "Changes", "Proof"]) expect(chrome[2]).toContain(tab);
  expect(chrome[2]).not.toContain("Inbox");
  // The host's binding reaches the panels: a disconnected host's status bar reads `○ unbound`, a bound one `● bound`.
  expect(panels.workbench.sideSource().bound).toBe(bound);
  // No agents, jobs, plan or failed read: the collapsed row carries no summary, so it is one row set into its own rule.
  expect(inline).toHaveLength(1);
  const row = stripTerminalSequences(inline[0] ?? "");
  expect(row.startsWith("─ Context  Files  Plan ─")).toBeTrue(); expect(row.trimEnd().endsWith("▴")).toBeTrue();
  expect(visibleWidth(inline[0] ?? "")).toBe(80);
  // The model label belongs to the real composer, not the header or the sidebar.
  expect([...chrome, ...rail].join("\n")).not.toContain("gpt-6.1-sol");
}

test("the host's disconnected state reaches its header and responsive sidebar without model calls", () => captureInspector(false));

test("input is rejected before local startup, which binds and persists the active transcript", async () => {
  expect(await submit("Reply only EARLY.", "prompt")).toEqual({ disposition: "rejected", reason: "host_not_ready" });
  await host.startLocal();
  expect(host.bound).toBeTrue();
  captureInspector(true);
  const file = host.session.sessionFile;
  if (!file) throw new Error("the locally bound host has no transcript");
  expect(file.startsWith(join(work, "sessions"))).toBeTrue();
  expect(existsSync(file)).toBeTrue();
  expect(host.state().session).toMatchObject({ bound_id: host.sessionId(), file });
  expect(events.since(0, 64 * 1024).events.filter(event => event.kind === "bound")).toMatchObject([
    { session_id: host.sessionId() },
  ]);
});

test("idle input starts a turn and emits local lifecycle events", async () => {
  const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 1);
  expect(await submit("Reply only STARTED.", "prompt")).toEqual({ disposition: "started", turn: 1 });
  await ended;
  const lifecycle = events.since(0, 64 * 1024).events.filter(event => event.kind === "turn_started" || event.kind === "turn_ended");
  expect(lifecycle).toMatchObject([
    { kind: "turn_started", turn: 1, origin: { kind: "bridge" }, provider: "openai-codex", model: "gpt-6.1-sol" },
    { kind: "turn_ended", turn: 1, cancelled: false },
  ]);
});

test("busy input is queued for steer and follow_up, rejected for prompt, and never claims completion", async () => {
  const transport = fixtureTransport();
  expect(await submit("/fixture-hold", "prompt")).toEqual({ disposition: "handled" });
  const held = transport.whenHeld();
  expect(await submit("Reply only HELD.", "prompt")).toEqual({ disposition: "started", turn: 2 });
  await within(held, 30_000, "the held response");
  expect(await submit("Reply only STEERED.", "steer")).toEqual({ disposition: "queued" });
  expect(await submit("Reply only FOLLOWED.", "follow_up")).toEqual({ disposition: "queued" });
  expect(await submit("Reply only BUSY.", "prompt")).toEqual({ disposition: "rejected", reason: "busy" });
  expect(host.session.getSteeringMessages()).toEqual(["Reply only STEERED."]);
  expect(host.session.getFollowUpMessages()).toEqual(["Reply only FOLLOWED."]);
  expect(host.state().streaming).toBeTrue();
  expect(host.state().turn).toEqual({ current: 2, open: true });
  const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 2);
  expect(await submit("/fixture-release", "prompt")).toEqual({ disposition: "handled" });
  await ended;
  expect(host.session.getSteeringMessages()).toEqual([]);
  expect(host.session.getFollowUpMessages()).toEqual([]);
});

test("cancel aborts the current turn only", async () => {
  const transport = fixtureTransport();
  expect(await host.cancel(undefined)).toEqual({ cancelled: false, reason: "idle" });
  await submit("/fixture-hold", "prompt");
  const held = transport.whenHeld();
  expect(await submit("Reply only CANCEL.", "prompt")).toEqual({ disposition: "started", turn: 3 });
  await within(held, 30_000, "the held response");
  expect(await host.cancel(99)).toEqual({ cancelled: false, reason: "turn_not_current", turn: 3 });
  const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 3);
  expect(await host.cancel(3)).toEqual({ cancelled: true, turn: 3 });
  expect((await ended).cancelled).toBeTrue();
});

test("local events have an increasing sequence and one lifecycle pair per turn", () => {
  const page = events.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER);
  const seqs = page.events.map((event) => event.seq);
  expect(seqs).toEqual([...seqs].sort((a, b) => a - b));
  expect(new Set(seqs).size).toBe(seqs.length);
  const turns = page.events.filter(event => event.kind === "turn_started" || event.kind === "turn_ended");
  expect(turns.map(event => [event.kind, event.turn])).toEqual([
    ["turn_started", 1], ["turn_ended", 1], ["turn_started", 2], ["turn_ended", 2], ["turn_started", 3], ["turn_ended", 3],
  ]);
});

test("input held before upstream preflight is never reported rejected, and its turn keeps the bridge origin", async () => {
  const held = holdNextInput();
  let reported: unknown;
  const submitted = submit("HOLD_INPUT Reply only LATE.", "prompt");
  void submitted.then((result) => { reported = result; });
  await within(held.entered, 30_000, "upstream to hold the input");
  expect(reported).toBeUndefined();
  const started = nextEvent(events, (event) => event.kind === "turn_started");
  const ended = nextEvent(events, (event) => event.kind === "turn_ended");
  const turn = (host.state().turn as { current: number }).current + 1;
  held.release();
  expect(await submitted).toEqual({ disposition: "started", turn });
  expect((await started).origin).toEqual({ kind: "bridge" });
  await ended;
});

test("through the bridge, input still before preflight at the reply deadline is pending, then settles started with its origin", async () => {
  const server = await startBridge({
    path: join(work, "e2e.sock"), sessionId: "fixture", events, inputReplyMs: 50,
    handlers: testHandlers({ submit: submission => host.submitLocal(submission) }),
  });
  const client = await connect(join(work, "e2e.sock"));
  const held = holdNextInput();
  const args = { text: "HOLD_INPUT Reply only DEFERRED.", busy_policy: "prompt", origin: "owner" };
  const frame = { version: 2, id: "rq-deferred", command: "submit_input", args };
  const first = client.request(frame);
  await within(held.entered, 30_000, "upstream to hold the input");
  const reply = (await first).result;
  expect([reply?.disposition, reply?.reason]).toEqual(["pending", "preflight_pending"]);
  const settled = nextEvent(events, (event) => event.kind === "input_settled" && event.request_id === frame.id);
  const started = nextEvent(events, (event) => event.kind === "turn_started");
  const ended = nextEvent(events, (event) => event.kind === "turn_ended");
  held.release();
  const outcome = await settled;
  expect(outcome.disposition).toBe("started");
  expect((await started).origin).toEqual({ kind: "bridge" });
  expect((await started).turn).toBe(outcome.turn);
  await ended;
  expect(events.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events
    .filter((event) => event.request_id === frame.id && event.disposition === "rejected")).toHaveLength(0);
  client.close();
  await server.close();
});

async function expectRestricted(): Promise<void> {
  const state = host.state();
  expect([...(state.active_tools as string[])].sort()).toEqual(["agent_message", "agent_roster", "read"]);
  const resources = state.resources as { extensions: string[]; skills: number; prompts: number; context_files: number };
  expect(resources.skills).toBe(0);
  expect(resources.prompts).toBe(0);
  expect(resources.context_files).toBe(0);
  expect(resources.extensions).toHaveLength(1);
  for (const builtin of BUILTIN_EXTENSIONS) expect(resources.extensions.join(" ")).not.toContain(builtin);
  expect(state.settings).toEqual({
    cacheWarming: "off", enableInstallTelemetry: false, enableAnalytics: false, enableSkillCommands: false,
    quietStartup: true, showCacheMissNotices: true, transport: "sse", extensions: [], compaction: { enabled: true },
    steeringMode: "all", followUpMode: "all",
  });
  for (const tool of ["fixture_forbidden", "bash"]) {
    await submit(`/fixture-call ${tool}`, "prompt");
    const ended = nextEvent(events, (event) => event.kind === "turn_ended");
    expect((await submit("Reply only CALL.", "prompt")).disposition).toBe("started");
    await ended;
    const result = host.session.messages.findLast((message) => message.role === "toolResult") as
      { toolName: string; isError: boolean; content: { text?: string }[] } | undefined;
    expect(result?.toolName).toBe(tool);
    expect(result?.isError).toBeTrue();
    expect(result?.content[0]?.text).toBe(`Tool ${tool} not found`);
  }
  expect(forbiddenRuns()).toBe(0);
  let refused: unknown;
  try {
    await fetch("https://pi.dev/api/report-install?version=1.3.0");
  } catch (error) {
    refused = error;
  }
  expect(refused instanceof NetworkRefused).toBeTrue();
}

test("only the role loadout is active, before and after reload", async () => {
  await expectRestricted();
  expect((await submit("/fixture-reload", "prompt")).disposition).toBe("handled");
  await expectRestricted();
  const network = host.state().network as { refused: number; allowed: number };
  expect(network.refused).toBe(2);
  expect(network.allowed).toBeGreaterThan(0);
}, 60_000);

test("while bound, every native session replacement is refused and later delivery and tool calls stay in the bound file", async () => {
  const state = host.state();
  if (typeof state.session !== "object" || !state.session || !("file" in state.session)) throw new Error("missing session state");
  const boundFile = state.session.file;
  if (typeof boundFile !== "string") throw new Error("the locally bound host has no transcript");
  expect(host.session.sessionFile).toBe(boundFile);
  // Historical inspection uses the panel's read channel, not Pi's replacement APIs.
  const before = host.state(), bytes = readFileSync(boundFile, "utf8");
  const history = historyUI([0, 0, 0], ["t", "\x1b"]);
  const draft = history.ui.getEditorText();
  await host.panels?.workbench.history.open(history.ui);
  expect(history.frames.join("\n")).toContain("Historical input");
  expect(history.ui.getEditorText()).toBe(draft);
  expect(host.state().identity).toEqual(before.identity);
  expect(host.state().role).toEqual(before.role);
  expect(host.state().session).toEqual(before.session);
  expect(readFileSync(boundFile, "utf8")).toBe(bytes);
  // When the owner attempts /new, /resume, /fork and import after browsing:
  await expectReplacementsRefused(host, events);
  // Then a delivery and a tool call made afterwards are written to the bound file, after its marker.
  await submit("/fixture-call read", "prompt");
  const ended = nextEvent(events, (event) => event.kind === "turn_ended");
  expect((await submit("Reply only AFTER.", "prompt")).disposition).toBe("started");
  await ended;
  const entries = readFileSync(boundFile, "utf8").trim().split("\n").map((line) => JSON.parse(line));
  const marker = entries.findIndex(entry => entry.customType === "corral_host_started");
  const delivered = entries.findIndex(entry => entry.type === "message" && entry.message.role === "user"
    && JSON.stringify(entry.message.content).includes("Reply only AFTER."));
  const called = entries.findIndex((entry, index) => index > delivered && entry.type === "message"
    && entry.message.role === "assistant"
    && entry.message.content.some((item: { type: string; name?: string }) => item.type === "toolCall" && item.name === "read"));
  expect(marker).toBeGreaterThanOrEqual(0);
  expect(delivered).toBeGreaterThan(marker);
  expect(called).toBeGreaterThan(delivered);
  expect((host.state().session as { created: number }).created).toBe(1);
}, 60_000);
