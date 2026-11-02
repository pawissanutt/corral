import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { visibleWidth } from "@earendil-works/pi-tui";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { startPanelHost, type PanelHost } from "./support/panel-host.ts";
import { nextEvent, within } from "./support/wait.ts";

// T-688: stable reading, focus and compact paging in the production panel shell (real host, editor and transcript).
const KEY = { f6: "\x1b[17~", esc: "\x1b", pageUp: "\x1b[5~", pageDown: "\x1b[6~", top: "\x1b[1;5H", bottom: "\x1b[1;5F",
  down: "\x1b[B", left: "\x1b[D", right: "\x1b[C", ctrlB: "\x02" };
const GRAPHEMES = "界面 👩‍💻 café ｗｉｄｅ";
const paragraph = (index: number) => {
  const id = `P${String(index).padStart(2, "0")}`;
  return `${id} ${"reading anchor text ".repeat(9).trim()}${index % 6 === 3 ? " " + GRAPHEMES : ""} ${id}E`;
};
const BODY = Array.from({ length: 40 }, (_, i) => paragraph(i + 1)).join("\n\n");
const TAIL = "\n\n" + Array.from({ length: 8 }, (_, i) => paragraph(i + 41)).join("\n\n") + "\n\nLIVE_TAIL";
const DRAFT = `Draft ${GRAPHEMES} stays`;

const mainColumns = (h: PanelHost) => h.physical.columns - (h.panels.wide ? 35 : 0);
function rows(h: PanelHost): string[] {
  const screen = h.screen(), main = mainColumns(h);
  return Array.from({ length: screen.rows }, (_, row) => screen.slice(row, 0, main));
}
/** The paragraph a reader sees at the top of the transcript: the first non-blank row below the 2-row view bar. */
function readingAnchor(h: PanelHost): number {
  const lines = rows(h);
  let top = 2;
  while (top < lines.length && !lines[top]?.trim()) top++;
  for (let row = top; row < lines.length; row++) {
    const marker = /P(\d\d)(E| )/.exec(lines[row] ?? "");
    if (!marker) continue;
    const id = Number(marker[1]);
    return marker[2] === "E" || row === top ? id : id - 1;
  }
  throw new Error("no paragraph visible:\n" + lines.join("\n"));
}
function frame(h: PanelHost, ready: (screen: string) => boolean, what: string): Promise<void> {
  return h.physical.whenWritten(() => ready(rows(h).join("\n")), what);
}
function expectIntact(h: PanelHost): void {
  const screen = h.screen();
  expect(screen.overflow).toBe(0);
  if (!h.panels.wide) return;
  for (let row = 0; row < screen.rows; row++) {
    expect("┌│└").toContain(screen.slice(row, 106, 107));
    expect("┐│┘").toContain(screen.slice(row, 139, 140));
  }
}
function pager(h: PanelHost): { row: number; first: number; total: number; previous: number; next: number } | undefined {
  const screen = h.screen();
  for (let row = 0; row < screen.rows; row++) {
    const line = screen.slice(row, 0, screen.columns);
    const match = /‹ (\d+)(?:–\d+)?\/(\d+) ›/.exec(line);
    if (!match) continue;
    const previous = visibleWidth(line.slice(0, match.index));
    return { row, first: Number(match[1]), total: Number(match[2]), previous, next: previous + visibleWidth(match[0]) - 1 };
  }
  return undefined;
}
const click = (h: PanelHost, col: number, row: number) => {
  h.physical.send(`\x1b[<0;${col + 1};${row + 1}M`); h.physical.send(`\x1b[<0;${col + 1};${row + 1}m`);
};

test("follow pauses for reading through a stream and relocation; jump-to-live is explicit and keeps focus", async () => {
  const h = await startPanelHost();
  try {
    let release: () => void = () => {}, reached: () => void = () => {};
    const held = new Promise<void>(resolve => { release = resolve; });
    const first = new Promise<void>(resolve => { reached = resolve; });
    fixtureTransport().script([{ chunks: [BODY, TAIL], afterFirstChunk: () => { reached(); return held; } }]);
    for (const char of "Stream the reading fixture.") h.physical.send(char);
    h.physical.send("\r");
    await within(first, 10_000, "the first streamed delta");
    await frame(h, screen => screen.includes("P40E"), "the live end of the first delta");
    expect(h.transcript.isFollowingEnd).toBeTrue();

    // Reading back during the stream pauses follow; the SDK's jump-to-live control appears.
    h.physical.send(KEY.top); h.physical.send(KEY.pageDown); h.physical.send(KEY.pageDown);
    await frame(h, screen => screen.includes("Jump to latest message"), "the jump-to-live control");
    expect(h.transcript.isFollowingEnd).toBeFalse();
    const anchor = readingAnchor(h), scrollTop = h.transcript.scrollTop;
    expect(anchor).toBeGreaterThan(5);
    for (const char of DRAFT) h.physical.send(char);
    await frame(h, screen => screen.includes(DRAFT), "the grapheme draft in the real editor");

    const ended = nextEvent(h.events, event => event.kind === "turn_ended");
    release(); await ended; await h.host.drained();
    await h.physical.whenWritten(() => h.transcript.render(mainColumns(h)).some(line => line.includes("LIVE_TAIL")), "the finished stream");
    h.resize(140, 46);
    await frame(h, screen => screen.includes(DRAFT), "a settled frame after the stream");
    expect(h.transcript.scrollTop).toBe(scrollTop);
    expect(h.transcript.isFollowingEnd).toBeFalse();
    expect(rows(h).join("\n")).not.toContain("LIVE_TAIL");
    expect(readingAnchor(h)).toBe(anchor);

    // Width changes (narrow, minimum, wide, manual rail toggle) keep the reading position and the draft.
    for (const [columns, height] of [[80, 34], [52, 24], [140, 46]] as const) {
      h.resize(columns, height);
      await frame(h, screen => screen.includes(DRAFT.slice(0, 8)) && screen.includes("Jump to latest"), `${columns}x${height} reading frame`);
      expect(readingAnchor(h)).toBe(anchor);
      expect(h.transcript.isFollowingEnd).toBeFalse();
      expect(h.host.sessionId()).toBe(h.bound);
      expectIntact(h);
    }
    for (const wide of [false, true]) {
      const mark = h.physical.writes.length;
      h.physical.send(KEY.ctrlB);
      await h.physical.whenWritten(() => h.panels.wide === wide && h.physical.writes.length > mark, "manual rail toggle frame");
      await frame(h, screen => screen.includes("Jump to latest"), "manual relocation frame");
      expect(readingAnchor(h)).toBe(anchor);
      expectIntact(h);
    }

    // Narrow keyboard route: viewport keys scroll the transcript but never take focus from the inspector.
    h.resize(80, 34);
    await frame(h, screen => screen.includes("F6 inspect"), "narrow frame");
    h.physical.send(KEY.f6);
    expect(h.panels.focused).toBeTrue();
    h.physical.send(KEY.pageUp);
    expect(h.panels.focused).toBeTrue();
    expect(h.transcript.isFollowingEnd).toBeFalse();
    h.physical.send(KEY.bottom);
    await frame(h, screen => screen.includes("LIVE_TAIL"), "jump to live by keyboard");
    expect(h.transcript.isFollowingEnd).toBeTrue();
    expect(h.panels.focused).toBeTrue();

    // Wide pointer route: wheel pauses follow; clicking the jump control resumes it, focus unchanged.
    h.resize(140, 46);
    await frame(h, screen => screen.includes("LIVE_TAIL"), "wide live frame");
    expect(h.panels.focused).toBeTrue();
    for (let i = 0; i < 6; i++) h.physical.send("\x1b[<64;20;12M");
    await frame(h, screen => screen.includes("Jump to latest message"), "wheel pauses follow");
    expect(h.transcript.isFollowingEnd).toBeFalse();
    const jump = h.screen().find("Jump to latest message");
    if (!jump) throw new Error("jump control not on screen");
    click(h, jump.col + 2, jump.row);
    await frame(h, screen => screen.includes("LIVE_TAIL") && !screen.includes("Jump to latest message"), "jump to live by pointer");
    expect(h.transcript.isFollowingEnd).toBeTrue();
    expect(h.panels.focused).toBeTrue();
    expect(h.screen().text()).toContain(DRAFT);
    expectIntact(h);
  } finally {
    await h.stop();
  }
  expect(h.physical.writes.join("")).toContain("\x1b[?1049l");
  expect(h.panels.canInteract).toBeFalse();
}, 30_000);

test("short terminals page metadata with agreeing keyboard and pointer routes, isolated from dialogs, across relocation", async () => {
  let answer: boolean | undefined;
  const h = await startPanelHost(pi => {
    pi.registerCommand("shell-confirm", { description: "Owned fixture confirmation", handler: async (_args, ctx) => {
      answer = await ctx.ui.confirm("Shell confirmation", "Cancel this owned fixture prompt.");
    } });
  });
  try {
    for (const char of DRAFT) h.physical.send(char);
    h.resize(80, 14);
    await frame(h, screen => screen.includes("F6 inspect"), "short narrow frame");
    h.physical.send(KEY.f6);
    await h.physical.whenWritten(() => pager(h) !== undefined, "compact inspector pager");
    const start = pager(h);
    if (!start) throw new Error("pager missing");
    expect(start.first).toBe(1);
    expect(start.total).toBeGreaterThan(1);

    h.physical.send(KEY.down);
    await h.physical.whenWritten(() => pager(h)?.first === 2, "keyboard next item");
    click(h, start.previous, start.row);
    await h.physical.whenWritten(() => pager(h)?.first === 1, "pointer previous item");
    click(h, start.next, start.row);
    await h.physical.whenWritten(() => pager(h)?.first === 2, "pointer next item");
    const reading = h.screen().slice(start.row + 1, 0, 80).trim();
    expect(reading.length).toBeGreaterThan(0);

    // A foreground dialog owns input: pager clicks and inspector keys change nothing underneath it.
    const confirmed = h.host.session.prompt("/shell-confirm", { source: "interactive" });
    await frame(h, screen => screen.includes("Shell confirmation"), "dialog frame");
    expect(h.panels.focused).toBeFalse(); // The prompt owns input focus while it is open.
    const offset = h.panels.offset;
    click(h, start.next, start.row); h.physical.send(KEY.f6); h.physical.send(KEY.ctrlB); h.physical.send(KEY.down);
    expect(h.panels.focused).toBeFalse();
    expect(h.panels.expanded).toBeFalse();
    expect(h.panels.offset).toBe(offset);
    h.physical.send(KEY.esc);
    await within(confirmed, 10_000, "dialog cancellation");
    expect(answer).toBeFalse();
    await h.physical.whenWritten(() => h.panels.canInteract, "the dialog to release input");
    h.physical.send(KEY.f6);
    await h.physical.whenWritten(() => pager(h)?.first === 2, "the same item after the dialog");

    // Relocation to the wide rail, and the manual inline toggle, keep tab, item, focus, draft and binding.
    const tab = h.panels.tab;
    h.resize(140, 46);
    await frame(h, screen => screen.includes(reading), "rail shows the same item");
    expect(h.panels.wide).toBeTrue();
    expect(h.panels.tab).toBe(tab);
    expect(h.panels.focused).toBeTrue();
    h.physical.send(KEY.ctrlB);
    await h.physical.whenWritten(() => !h.panels.wide && rows(h).join("\n").includes(reading), "manual inline relocation");
    expect(h.screen().text()).toContain(DRAFT);
    expect(h.host.sessionId()).toBe(h.bound);
    expectIntact(h);
  } finally {
    await h.stop();
  }
}, 30_000);

test("the plain host keeps its fallback: no native panels are created", async () => {
  const work = mkdtempSync(join(tmpdir(), "corral-panel-plain-")), assets = buildFixtureInstall();
  const oldFetch = globalThis.fetch;
  let host: HostSession | undefined;
  try {
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 1 };
    const modules = await loadModules(assets.dir, "worker");
    const send = modules.auth.transport?.(authContext());
    if (!send) throw new Error("fixture transport unavailable");
    const network = guardedFetch(send);
    globalThis.fetch = network.fetch as typeof globalThis.fetch;
    const client = new CorralClient({ corral: writeFakeCorral(work).path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    host = await HostSession.create({ sdk, identity, cwd: work, agentDir: join(work, "agent"), model: "gpt-6.1-sol", modules, client,
      events: new EventLog(1), network, ui: "plain", sessionManager: sdk.SessionManager.create(work, join(work, "sessions")) });
    expect(host.panels).toBeUndefined();
  } finally {
    await host?.close(); assets.cleanup(); rmSync(work, { recursive: true, force: true }); globalThis.fetch = oldFetch;
  }
});
