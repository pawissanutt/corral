import { expect, test } from "bun:test";
import { getKeybindings } from "@earendil-works/pi-tui";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { startPanelHost, type PanelHost } from "./support/panel-host.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { nextEvent, within } from "./support/wait.ts";

const type = (h: PanelHost, text: string) => { for (const char of text) h.physical.send(char); };
const frame = async (h: PanelHost, text: string) => {
  try { await h.physical.whenWritten(() => h.screen().text().includes(text), text); }
  catch (error) { throw new Error(`${String(error)}\n${h.screen().text()}\nqueues: ${JSON.stringify([h.host.session.getSteeringMessages(), h.host.session.getFollowUpMessages()])}`); }
  expect(h.screen().overflow).toBe(0);
  const evidence = process.env.CORRAL_COMPOSER_EVIDENCE;
  if (evidence) {
    mkdirSync(evidence, { recursive: true });
    const name = text.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
    writeFileSync(join(evidence, `${name || "control"}.ansi`), h.physical.writes.join(""));
    writeFileSync(join(evidence, `${name || "control"}.txt`), h.screen().text());
  }
};
const click = (h: PanelHost, label: string) => {
  const hit = h.screen().find(label);
  if (!hit) throw new Error(`missing control: ${label}\n${h.screen().text()}`);
  h.physical.send(`\x1b[<0;${hit.col + 1};${hit.row + 1}M`);
  h.physical.send(`\x1b[<0;${hit.col + 1};${hit.row + 1}m`);
};

test("real keyboard input reports pending, started and rejected admission without mistaking completion", async () => {
  let unblock: () => void = () => {}, entered: () => void = () => {}, finish: () => void = () => {}, streamed: () => void = () => {};
  const heldInput = new Promise<void>(resolve => { unblock = resolve; });
  const inputSeen = new Promise<void>(resolve => { entered = resolve; });
  const heldReply = new Promise<void>(resolve => { finish = resolve; });
  const firstChunk = new Promise<void>(resolve => { streamed = resolve; });
  const h = await startPanelHost(pi => pi.on("input", async event => {
    if (event.text === "Typed café") { entered(); await heldInput; }
    if (event.text === "Handled this") return { action: "handled" as const };
  }));
  try {
    fixtureTransport().script([{ chunks: ["PARTIAL", " COMPLETE"], afterFirstChunk: () => { streamed(); return heldReply; } }]);
    type(h, "Typed cafx"); h.physical.send("\x7f"); type(h, "é"); h.physical.send("\r");
    await within(inputSeen, 10_000, "input preflight");
    await frame(h, "Last typed input: pending");
    expect(h.host.session.messages.filter(m => m.role === "user")).toHaveLength(0);
    unblock(); await within(firstChunk, 10_000, "first delta");
    await frame(h, "Last typed input: started");
    expect(h.screen().text()).toContain("not completion");
    expect(h.host.state().turn).toEqual({ current: 1, open: true });
    const ended = nextEvent(h.events, e => e.kind === "turn_ended"); finish(); await ended; await h.host.drained();
    await frame(h, "COMPLETE");
    expect(h.host.session.messages.filter(m => m.role === "user")).toHaveLength(1);
    type(h, "Handled this"); h.physical.send("\r"); await frame(h, "Last typed input: handled");
    h.host.runtime.services.modelRuntime.hasConfiguredAuth = () => false;
    h.host.runtime.services.modelRuntime.checkAuth = async () => undefined;
    h.host.runtime.services.modelRuntime.isUsingOAuth = () => false;
    type(h, "Reject this"); h.physical.send("\r");
    await frame(h, "Last typed input: rejected");
    expect(h.screen().text()).toContain("No API key");
    expect(h.host.session.messages.filter(m => m.role === "user")).toHaveLength(1);
    expect(h.host.sessionId()).toBe(h.bound);
  } finally { unblock(); finish(); await h.stop(); }
}, 30_000);

test("busy Enter steers and Alt+Enter follows once; notices stay read-only and drafts survive views/resize", async () => {
  const h = await startPanelHost();
  const keys = getKeybindings(), bindings = keys.getUserBindings();
  keys.setUserBindings({ ...bindings, "app.message.followUp": "alt+enter", "app.message.dequeue": "alt+up" });
  let finish: () => void = () => {}, streamed: () => void = () => {};
  const held = new Promise<void>(resolve => { finish = resolve; });
  const first = new Promise<void>(resolve => { streamed = resolve; });
  try {
    fixtureTransport().script([{ chunks: ["BUSY", " DONE"], afterFirstChunk: () => { streamed(); return held; } }]);
    type(h, "Start"); h.physical.send("\r"); await within(first, 10_000, "held stream");
    type(h, "Steer once"); h.physical.send("\r"); await frame(h, "Last typed input: queued");
    type(h, "Follow once"); h.physical.send("\x1b\r");
    await frame(h, "1 steering · 1 follow-ups");
    expect(h.host.session.getSteeringMessages()).toEqual(["Steer once"]);
    expect(h.host.session.getFollowUpMessages()).toEqual(["Follow once"]);
    const model = h.host.session.model, leaf = h.host.session.sessionManager.getLeafId();
    h.physical.send("\x10"); await frame(h, "model_switch_refused");
    expect(h.host.session.model).toEqual(model);
    expect(h.host.session.sessionManager.getLeafId()).toBe(leaf);
    for (let i = 0; i < 12; i++) await h.host.submit({ text: `STRUCTURED_WAKE_${i}`, busyPolicy: "follow_up", idempotencyKey: `wake-${i}`, origin: "corral-inbox" }, `request-${i}`);
    await frame(h, "13 follow-ups");
    expect(h.screen().text()).not.toContain("STRUCTURED_WAKE_");
    expect(h.screen().text()).not.toContain("edit all queued");
    type(h, "Draft 界面 stays"); h.physical.send("\x1b[1;3A"); // Native dequeue is not an owner-text source.
    await frame(h, "Queued input is read-only");
    expect(h.host.session.getFollowUpMessages()).toHaveLength(13);
    for (const key of ["\x1b[18~", "\x1b[19~", "\x1b[20~"]) h.physical.send(key);
    for (const [cols, rows] of [[80, 34], [52, 24], [140, 46]] as const) {
      h.resize(cols, rows); await frame(h, "Draft 界面 stays");
      expect(h.screen().text()).not.toContain("STRUCTURED_WAKE_");
    }
    expect(h.host.sessionId()).toBe(h.bound);
    // Stop only the current run. It must not transfer structured queued text into the editor.
    h.physical.send("\x1b"); await h.host.session.waitForIdle();
    await frame(h, "Draft 界面 stays");
    expect(h.screen().text()).not.toContain("STRUCTURED_WAKE_");
    expect(h.host.session.getSteeringMessages()).toEqual(["Steer once"]);
    expect(h.host.session.getFollowUpMessages()).toHaveLength(13);
  } finally { keys.setUserBindings(bindings); finish(); await h.stop(); }
}, 30_000);

test("composer pointer uses the native model picker and supported effort cycle without consuming a draft", async () => {
  const h = await startPanelHost();
  try {
    type(h, "Unsent owner draft");
    await frame(h, " ▾"); click(h, "gpt-6.1-sol");
    await frame(h, "Only showing models from configured providers");
    const session = h.host.session, file = session.sessionFile, original = session.model;
    const available = await h.host.runtime.services.modelRuntime.getAvailable();
    expect(new Set(available.map(model => model.provider))).toEqual(new Set(["openai-codex"]));
    const target = available.find(model => model.id !== original?.id && model.reasoning);
    if (!target) throw new Error("fixture requires a second reasoning model");
    type(h, target.id); h.physical.send("\r");
    await h.physical.whenWritten(() => session.model?.id === target.id, "native picker commit");
    await frame(h, "Unsent owner draft");
    expect(session.sessionFile).toBe(file);
    const levels = h.host.session.getAvailableThinkingLevels(), before = h.host.session.thinkingLevel;
    click(h, " ↻");
    await h.physical.whenWritten(() => h.host.session.thinkingLevel !== before, "native effort cycle");
    expect(levels).toContain(h.host.session.thinkingLevel);
    h.physical.send("\x1b[Z");
    await frame(h, "Unsent owner draft");
    expect(levels).toContain(h.host.session.thinkingLevel);
    expect(h.host.sessionId()).toBe(h.bound);
    expect(h.host.session.messages.filter(m => m.role === "user")).toHaveLength(0);
    // The same native picker still goes through T-641 credential preflight, without committing a change.
    h.host.runtime.services.modelRuntime.getAuth = async () => undefined;
    const leaf = session.sessionManager.getLeafId();
    h.physical.send("\x0c"); await frame(h, "Only showing models from configured providers");
    type(h, original!.id); h.physical.send("\r");
    await frame(h, "credentials_unavailable");
    expect(h.screen().text()).toContain("corral agent login");
    expect(session.model?.id).toBe(target.id);
    expect(session.sessionManager.getLeafId()).toBe(leaf);
    expect(h.host.session).toBe(session);
    expect(h.screen().text()).toContain("Unsent owner draft");
  } finally { await h.stop(); }
}, 30_000);
