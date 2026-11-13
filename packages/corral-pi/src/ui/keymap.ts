// What each focus context offers, as footer hints and as the F1 keys list. Pi-owned keys come from the
// configured keybindings, so a changed user binding changes the hint; panel keys are fixed.
import { getKeybindings, visibleWidth, type KeyId, type Keybinding } from "@earendil-works/pi-tui";
import type { ChromeTier } from "./header.ts";
import { keyCap } from "./frame.ts";
import { pad, PANEL_BG, PANEL_MUTED, PANEL_TEXT, plainLabel, RESET } from "./panels-view.ts";

export type KeyContext = "composer" | "agents" | "agent" | "changes" | "task" | "lifecycle" | "proof" | "artifacts"
  | "inbox" | "usage" | "image" | "sidebar" | "queue" | "dialog" | "palette" | "keys" | "pager" | "board" | "choice";
export interface Hint { readonly id: string; readonly key: string; readonly label: string; readonly short?: string; readonly data?: string }
export interface HintState {
  readonly context: KeyContext;
  readonly mode: { readonly taskBound: boolean; readonly answer: boolean; readonly lifecycle: boolean };
  readonly streaming: boolean;
  readonly queue: { readonly items: number; readonly editable: number; readonly control: boolean };
  /** AgentsAction ids offered for the selected row; in `board`, the BoardAction ids offered. */
  readonly selection?: readonly string[];
  readonly authors?: boolean; readonly legend?: boolean; readonly history?: boolean; readonly timelineSource?: "task" | "host";
  /** Build sessions only: who Enter reaches, whether a child can take a message, a waiting board plan, docked options. */
  readonly build?: {
    readonly target?: { readonly id: string; readonly answer: boolean }; readonly steerable: boolean;
    readonly proposed: boolean; readonly choices: number;
  };
}
export interface FooterHit { readonly from: number; readonly to: number; readonly hint: Hint }
export interface KeyRow { readonly key: string; readonly label: string; readonly data?: string }
export interface KeyGroup { readonly title: string; readonly rows: readonly KeyRow[] }

const MODIFIERS: Readonly<Record<string, string>> = { ctrl: "Ctrl", alt: "Alt", shift: "Shift", super: "Super", meta: "Meta" };
const NAMES: Readonly<Record<string, string>> = {
  enter: "⏎", return: "⏎", escape: "esc", esc: "esc", up: "↑", down: "↓", left: "←", right: "→",
  pageup: "PgUp", pagedown: "PgDn", tab: "Tab", space: "Space", backspace: "Bksp", delete: "Del", insert: "Ins",
  home: "Home", end: "End",
};
/** `enter` → `⏎`, `escape` → `esc`, `alt+enter` → `Alt+⏎`, `alt+up` → `Alt+↑`, `ctrl+o` → `Ctrl+O`. */
export function keyLabel(binding: string): string {
  const parts = binding.split("+");
  const base = parts.pop() || "+";
  const mods = parts.map(part => MODIFIERS[part.toLowerCase()] ?? part);
  const lower = base.toLowerCase();
  const name = NAMES[lower] ?? (/^f\d{1,2}$/.test(lower) ? base.toUpperCase()
    : base.length === 1 ? (mods.length ? base.toUpperCase() : base) : base[0]!.toUpperCase() + base.slice(1));
  return [...mods, name].join("+");
}

const PLAIN: Readonly<Record<string, string>> = {
  enter: "\r", return: "\r", escape: "\x1b", esc: "\x1b", tab: "\t", space: " ", backspace: "\x7f",
};
const CURSOR: Readonly<Record<string, string>> = { up: "A", down: "B", right: "C", left: "D", home: "H", end: "F" };
const TILDE: Readonly<Record<string, number>> = {
  insert: 2, delete: 3, pageup: 5, pagedown: 6, f5: 15, f6: 17, f7: 18, f8: 19, f9: 20, f10: 21, f11: 23, f12: 24,
};
const SS3: Readonly<Record<string, string>> = { f1: "P", f2: "Q", f3: "R", f4: "S" };
/** The legacy terminal sequence for a key id, which a click replays; undefined when it has none. */
export function keyData(binding: string): string | undefined {
  const parts = binding.toLowerCase().split("+");
  const base = parts.pop() || "+";
  const shift = parts.includes("shift"), alt = parts.includes("alt"), ctrl = parts.includes("ctrl");
  if (parts.some(part => !(part in MODIFIERS)) || parts.includes("super") || parts.includes("meta")) return undefined;
  const modifier = 1 + (shift ? 1 : 0) + (alt ? 2 : 0) + (ctrl ? 4 : 0);
  if (CURSOR[base]) return modifier > 1 ? `\x1b[1;${modifier}${CURSOR[base]}` : `\x1b[${CURSOR[base]}`;
  if (TILDE[base] !== undefined) return modifier > 1 ? `\x1b[${TILDE[base]};${modifier}~` : `\x1b[${TILDE[base]}~`;
  if (SS3[base]) return modifier > 1 ? `\x1b[1;${modifier}${SS3[base]}` : `\x1bO${SS3[base]}`;
  if (base === "tab" && shift && !alt && !ctrl) return "\x1b[Z";
  const prefix = alt ? "\x1b" : "";
  if (PLAIN[base] !== undefined && !shift && !ctrl) return prefix + PLAIN[base];
  if (base.length !== 1) return undefined;
  if (ctrl) return /^[a-z]$/.test(base) && !shift ? prefix + String.fromCharCode(base.charCodeAt(0) - 96) : undefined;
  return prefix + (shift ? base.toUpperCase() : base);
}

// Pi binding ids the hints name, with the stock defaults used when no Pi binding table is installed.
const PI_DEFAULTS: Readonly<Record<string, string>> = {
  "tui.input.submit": "enter", "app.interrupt": "escape", "app.message.followUp": "alt+enter",
  "app.message.dequeue": "alt+up", "app.tools.expand": "ctrl+o", "app.model.select": "ctrl+l", "app.thinking.cycle": "shift+tab",
};
/** First configured key via getKeybindings(); undefined when the user unbound it. */
export function piKey(id: string): string | undefined {
  const keys = getKeybindings();
  if (!keys.getDefinition(id as Keybinding)) return PI_DEFAULTS[id];
  return keys.getKeys(id as Keybinding)[0];
}
// Bindings scoped to Pi's own selectors, where panel keys are inert, never collide with a panel key.
const SCOPED = ["app.models.", "app.tree.", "tui.select."];
/** `key` unless the user bound it to an editor or app action. */
function freeKey(key: KeyId): KeyId | undefined {
  for (const [id, bound] of Object.entries(getKeybindings().getResolvedBindings())) {
    if (SCOPED.some(prefix => id.startsWith(prefix))) continue;
    if ((Array.isArray(bound) ? bound : bound ? [bound] : []).some(item => item.toLowerCase() === key)) return undefined;
  }
  return key;
}
/** Alt+↓ focuses the queue unless the user bound it to an editor or app action. */
export function queueFocusKey(): KeyId | undefined {
  return freeKey("alt+down");
}
/** Alt+T changes who receives the message in a build session, unless the user bound it to an editor or app action. */
export function targetKey(): KeyId | undefined {
  return freeKey("alt+t");
}

function hint(id: string, binding: string, label: string, short?: string): Hint {
  const data = keyData(binding);
  return { id, key: keyLabel(binding), label, ...(short ? { short } : {}), ...(data ? { data } : {}) };
}
function pair(id: string, key: string, data: string, label: string): Hint {
  return { id, key, label, data };
}
function piHint(binding: string, id: string, label: string, short?: string): Hint | undefined {
  const key = piKey(binding);
  return key ? hint(id, key, label, short) : undefined;
}
const UP_DOWN = (id: string, label: string) => pair(id, "↑↓", "\x1b[B", label);
const J_K = (id: string, label: string) => pair(id, "j/k", "j", label);
const ESC = (label: string) => hint("escape", "escape", label);
const ENTER = (label: string) => hint("enter", "enter", label);
const letter = (key: string, label: string, short?: string) => hint(key, key, label, short);

const AGENT_HINTS: Readonly<Record<string, Hint>> = {
  open: ENTER("open"), answer: letter("a", "answer"), transcript: letter("t", "transcript", "log"), steer: letter("s", "steer"),
  cancel: letter("x", "cancel"), model: letter("m", "model"),
};
const JOB_HINTS: Readonly<Record<string, Hint>> = {
  output: ENTER("output"), stop: letter("x", "stop"), peek: letter("p", "peek"), follow: letter("f", "follow"),
  search: letter("/", "search"), background: letter("b", "background", "bg"),
};
const AGENT_ORDER = ["open", "answer", "transcript", "steer", "cancel", "model"];
const JOB_ORDER = ["output", "stop", "peek", "follow", "search", "background"];

function targetHint(): Hint | undefined {
  const key = targetKey();
  return key ? hint("target", key, "target", "to") : undefined;
}
function composerHints(state: HintState): (Hint | undefined)[] {
  const build = state.build, target = build?.target;
  if (target) {
    // A child receives Enter: the queue and follow-up keys stay the parent's, so only Alt+T and stop remain.
    const id = plainLabel(target.id);
    const submit = piHint("tui.input.submit", "submit", `${target.answer ? "answer" : "steer"} ${id}`, target.answer ? "answer" : "steer");
    return [submit, targetHint(), !target.answer && state.streaming ? piHint("app.interrupt", "interrupt", "stop") : undefined];
  }
  const to = build?.steerable ? targetHint() : undefined;
  // Pulling queued input back comes before stop: it is the key the mock's composer footer names.
  const [dequeue, focus] = queueHints(state);
  return state.streaming
    ? [piHint("tui.input.submit", "submit", "steer"), piHint("app.message.followUp", "followUp", "queue"), to, dequeue,
      piHint("app.interrupt", "interrupt", "stop"), focus]
    : [piHint("tui.input.submit", "submit", "send"), to, dequeue, focus];
}
function boardHints(state: HintState): (Hint | false | undefined)[] {
  // Without the view's offered actions, a task is assumed selected and the board reachable. Task keys need a task.
  const offered = new Set(state.selection ?? ["details", "refresh", ...(state.build?.proposed ? ["review"] : [])]);
  return [offered.has("details") && UP_DOWN("task", "task"), offered.has("review") && letter("p", "review plan", "plan"), offered.has("details") && ENTER("details"),
    offered.has("plan") && letter("n", "plan tasks", "plan"), offered.has("refresh") && letter("r", "refresh"), ESC("chat")];
}
function queueHints(state: HintState): (Hint | undefined)[] {
  // Without an editable item and queue control there is nothing the queue keys could do.
  if (!state.queue.control || state.queue.editable < 1) return [];
  const focus = queueFocusKey();
  return [piHint("app.message.dequeue", "dequeue", "edit queued", "edit"), focus ? hint("queue.focus", focus, "queue") : undefined];
}

/** The contextual hints of section 5.4, in priority order, without F2 and F1. */
export function contextHints(state: HintState): Hint[] {
  const pick = (...hints: (Hint | false | undefined)[]) => hints.filter((h): h is Hint => !!h);
  switch (state.context) {
    case "composer":
      return pick(...composerHints(state));
    case "agents": {
      const offered = new Set(state.selection ?? []);
      const job = JOB_ORDER.some(action => offered.has(action));
      const order = job ? JOB_ORDER : AGENT_ORDER, table = job ? JOB_HINTS : AGENT_HINTS;
      return pick(UP_DOWN("select", "select"), ...order.filter(action => offered.has(action)).map(action => table[action]), ESC("chat"));
    }
    case "agent": {
      const offered = new Set(state.selection ?? []);
      return pick(ESC("agents"), ...["answer", "steer", "cancel", "model", "transcript"].filter(action => offered.has(action)).map(action => AGENT_HINTS[action]),
        piHint("app.tools.expand", "tools", "tools"));
    }
    case "changes":
      return pick(J_K("file", "file"), pair("hunk", "n/p", "n", "hunk"), letter("a", "ask"), state.authors && letter("f", "by agent"),
        letter("/", "filter"), letter("c", "compare"), letter("o", "project"), letter("r", "refresh"), ESC("chat"));
    case "task":
      return pick(UP_DOWN("scroll", "scroll"), letter("r", "refresh"), state.mode.lifecycle && letter("g", "graph"),
        state.mode.lifecycle && letter("l", "events"), ESC("chat"));
    case "lifecycle":
      return pick(J_K("select", "select"), letter("r", "reconcile"),
        state.timelineSource === "task" && letter("h", "host"), state.timelineSource === "host" && letter("c", "canonical"), ESC("task"));
    case "proof":
      return pick(J_K("criterion", "criterion"), ENTER("open"), letter("a", "artifacts"), letter("?", state.legend ? "hide" : "legend"),
        letter("r", "refresh"), ESC("chat"));
    case "artifacts":
      return pick(J_K("select", "select"), ENTER("open"), letter("m", "mark"), letter("c", "compare"), letter("o", "browse"),
        letter("x", "export"), letter("r", "refresh"), ESC("proof"));
    case "inbox":
      return pick(J_K("select", "select"), ENTER("inspect"), state.mode.answer && letter("a", "answer flow", "answer"),
        letter("h", state.history ? "pending" : "history"), letter("/", "filter"), letter("r", "reread"), letter("?", "legend"), ESC("chat"));
    case "usage":
      return pick(UP_DOWN("scroll", "scroll"), letter("c", "chart"), letter("r", "refresh"), ESC("chat"));
    case "sidebar":
      return pick(pair("tab", "←→", "\x1b[C", "tab"), UP_DOWN("select", "select"), ENTER("open"), ESC("composer"));
    case "queue":
      return pick(UP_DOWN("select", "select"), ENTER("edit"), pair("move", "Shift+↑↓", "\x1b[1;2B", "move"), letter("x", "remove"), ESC("composer"));
    case "dialog":
      return pick(UP_DOWN("select", "select"), ENTER("choose"), ESC("cancel"));
    case "palette":
      return pick(UP_DOWN("select", "select"), ENTER("run"), ESC("close"));
    case "keys":
      return pick(ENTER("run"), ESC("close"));
    case "pager":
      return pick(UP_DOWN("scroll", "scroll"), pair("page", "PgUp/PgDn", "\x1b[6~", "page"), ESC("close"));
    case "image":
      return pick(ESC("close"), hint("f9", "f9", "chat"));
    case "board":
      return pick(...boardHints(state));
    case "choice": {
      // Digits pick directly; the hint names the range and replays nothing, so a click never picks blindly.
      const choices = state.build?.choices ?? 0;
      return pick(UP_DOWN("select", "select"), ENTER("choose"),
        choices > 0 && { id: "pick", key: choices > 1 ? `1-${Math.min(9, choices)}` : "1", label: "pick" }, ESC("not now"));
    }
  }
}

/** Dialogs, the docked choice dialog, the palette, the keys list and pagers own their keys: their footer has no F2 or F1. */
export function footerBase(context: KeyContext): boolean {
  return context !== "dialog" && context !== "palette" && context !== "keys" && context !== "pager" && context !== "choice";
}

function hintWidth(item: Hint, short: boolean): number {
  const label = short ? item.short ?? item.label : item.label;
  return visibleWidth(item.key) + 2 + (label ? 1 + visibleWidth(label) : 0);
}

/**
 * The footer row: contextual hints, then `F2 actions` (wide and mid) and `F1` when `base`. As many contextual
 * hints as fit the row show, in priority order, at any width; F2 goes before any of them, and short labels are
 * used when they let more hints show. F1 reads `keys` when nothing was dropped and `more` when something was;
 * at worst it is a bare cap.
 */
export function footerRow(hints: readonly Hint[], tier: ChromeTier, width: number, base: boolean):
  { readonly line: string; readonly hits: readonly FooterHit[] } {
  const roomy = tier === "wide" || tier === "mid";
  const gap = tier === "tiny" ? 1 : 2;
  const room = Math.max(0, width - 1);
  const f2 = base && roomy ? hint("f2", "f2", "actions") : undefined;
  const f1 = (label: string): Hint[] => base ? [hint("f1", "f1", label)] : [];
  const measure = (shown: readonly Hint[], short: boolean) =>
    shown.reduce((sum, item) => sum + hintWidth(item, short), 0) + gap * Math.max(0, shown.length - 1);
  /** The most contextual hints that fit with `tail` after them. */
  const most = (short: boolean, tail: readonly Hint[]) => {
    let count = hints.length;
    while (count > 0 && measure([...hints.slice(0, count), ...tail], short) > room) count--;
    return count;
  };
  let short = false;
  let shown: Hint[];
  if (measure([...hints, ...(f2 ? [f2] : []), ...f1("keys")], false) <= room) shown = [...hints, ...(f2 ? [f2] : []), ...f1("keys")];
  else {
    const full = most(false, f1("more")), brief = most(true, f1("more"));
    short = brief > full;
    const count = short ? brief : full;
    shown = [...hints.slice(0, count), ...f1(count === hints.length && !f2 ? "keys" : "more")];
    if (measure(shown, short) > room) shown = f1("");
  }
  const hits: FooterHit[] = [];
  const parts: string[] = [];
  let x = 1;
  for (const item of shown) {
    const label = short ? item.short ?? item.label : item.label;
    parts.push(keyCap(item.key) + (label ? " " + PANEL_MUTED + label : ""));
    const end = x + hintWidth(item, short);
    if (x < width) hits.push({ from: x, to: Math.min(width, end), hint: item });
    x = end + gap;
  }
  const line = " " + parts.join(PANEL_MUTED + " ".repeat(gap));
  return { line: PANEL_BG + PANEL_TEXT + pad(line, Math.max(0, width)).replaceAll(RESET, RESET + PANEL_BG) + RESET, hits };
}

/**
 * The F1 keys list for this mode: every group holds only keys the mode offers. Rows with data run on Enter.
 * `build` adds Board and the message receiver, names each agent's own keys, and has no Task, Proof or Inbox.
 */
export function keyGroups(mode: {
  readonly taskBound: boolean; readonly answer: boolean; readonly jobs: boolean;
  readonly queueControl: boolean; readonly lifecycle: boolean; readonly build?: boolean;
}): KeyGroup[] {
  const row = (binding: string, label: string): KeyRow => {
    const data = keyData(binding);
    return { key: keyLabel(binding), label, ...(data ? { data } : {}) };
  };
  const pi = (id: string, label: string): KeyRow | undefined => {
    const key = piKey(id);
    return key ? row(key, label) : undefined;
  };
  const info = (key: string, label: string): KeyRow => ({ key, label });
  const rows = (...items: (KeyRow | false | undefined)[]) => items.filter((item): item is KeyRow => !!item);
  const focus = mode.queueControl ? queueFocusKey() : undefined;
  const build = mode.build === true, target = build ? targetKey() : undefined;
  const taskBound = mode.taskBound && !build;
  const groups: KeyGroup[] = [
    { title: "Views", rows: rows(
      taskBound && row("f5", "Task: the bound task, its plan and criteria"),
      row("f9", "Chat: the conversation"),
      row("f8", build ? "Agents: every agent, focus the list" : mode.jobs ? "Agents: helper calls and jobs, focus the list" : "Agents: helper calls, focus the list"),
      row("f7", build ? "Changes: files and diffs, filtered by agent" : "Changes: files and diffs in the chosen comparison"),
      taskBound && row("f10", "Proof: criteria, gate and review"),
      build && row("f10", "Board: tasks this session filed"),
      !build && row("f11", "Inbox: decisions waiting for the owner"),
    ) },
    { title: "Message", rows: rows(
      pi("tui.input.submit", build ? "Send; steer while something runs; steers the chosen agent when one receives" : "Send; steer while the agent works"),
      pi("app.message.followUp", build ? "Queue a follow-up for the parent" : "Queue as a follow-up"),
      target !== undefined && row(target, "Change the receiver: parent or one running agent"),
      pi("app.interrupt", "Stop the running turn"),
      mode.queueControl && pi("app.message.dequeue", "Pull the last queued input back to edit"),
      pi("app.model.select", "Choose the model"),
      pi("app.thinking.cycle", "Cycle the effort"),
      pi("app.tools.expand", "Expand or fold tool output"),
    ) },
    { title: "Agents", rows: rows(
      info("↑↓", "Select an agent" + (mode.jobs ? " or job" : "")),
      info("⏎", mode.jobs ? "Open its conversation, or a job's output" : "Open its conversation"),
      build && info("a", "Answer its question"),
      build && info("s", "Point the composer at it"),
      build && info("x", "Cancel it (asks first); its edits stay" + (mode.jobs ? "; on a job, stops it" : "")),
      build && info("m", "Change its model from its next request"),
      info("t", "Read-only transcript, when the agent has one"),
      !build && mode.jobs && info("x", "Stop the selected job (asks first)"),
      mode.jobs && info("p", "Peek at the job's current logs"),
      mode.jobs && info("f", "Follow the job's output on or off"),
      mode.jobs && info("/", "Search the job's output"),
      mode.jobs && info("b", "Move the job to the background"),
      info("esc", "Back to Chat; from a conversation, back to Agents"),
    ) },
    { title: "Changes", rows: rows(
      info("j/k", "Next or previous file"),
      info("n/p", "Next or previous hunk"),
      info("a", "Ask about the hunk (added to the draft, not sent)"),
      info("/", "Filter paths"),
      info("c", "Choose the comparison"),
      info("o", "Choose the project or worktree"),
      build && info("f", "Filter by agent"),
      info("r", "Refresh"),
    ) },
  ];
  if (build) {
    groups.push({ title: "Board", rows: rows(
      info("↑↓", "Select a task"),
      info("⏎", "Details"),
      info("n", "Plan board tasks from this conversation"),
      info("p", "Review the proposed plan"),
      info("r", "Refresh status"),
    ) });
  }
  if (taskBound) {
    groups.push({ title: "Task", rows: rows(
      info("↑↓", "Scroll"),
      info("r", "Refresh"),
      mode.lifecycle && info("g", "Lifecycle graph"),
      mode.lifecycle && info("l", "Lifecycle events"),
      mode.lifecycle && info("h/c", "Events: host log or canonical record"),
    ) });
    groups.push({ title: "Proof", rows: rows(
      info("j/k", "Select a criterion"),
      info("⏎", "Open its detail"),
      info("a", "Artifacts"),
      info("?", "Show or hide the legend"),
      info("r", "Refresh"),
    ) });
  }
  if (!build) {
    groups.push({ title: "Inbox", rows: rows(
      info("j/k", "Select an item"),
      info("⏎", "Inspect it"),
      mode.answer && info("a", "Answer flow instructions (nothing is sent)"),
      info("h", "History or pending"),
      info("/", "Filter"),
      info("r", "Reread"),
      info("?", "Legend"),
    ) });
  }
  groups.push({ title: "Sidebar", rows: rows(
    row("ctrl+b", "Collapse to one row, or open it"),
    row("f6", "Focus the sidebar; again for the next tab"),
    info("←→", "Switch tab: Context, Files, Plan"),
    info("⏎", "Open the selected item"),
    info("esc", "Back to the composer"),
  ) });
  if (mode.queueControl) {
    groups.push({ title: "Queue", rows: rows(
      focus ? row(focus, "Focus the queue") : undefined,
      info("⏎", "Pull the item back to edit"),
      info("Shift+↑↓", "Move it earlier or later"),
      info("x", "Remove it; it is not sent"),
      info("esc", "Back to the composer"),
    ) });
  }
  groups.push({ title: "Anywhere", rows: rows(
    row("f2", "Search actions; unavailable ones say why"),
    info("F1", "This list; F1 or esc closes it"),
    info("esc", "Close an expanded image; a click on it closes it too"),
  ) });
  return groups.filter(group => group.rows.length > 0);
}
