// What each focus context offers, as footer hints and as the F1 keys list. Pi-owned keys come from the
// configured keybindings, so a changed user binding changes the hint; panel keys are fixed.
import { getKeybindings, matchesKey, visibleWidth, type KeyId, type Keybinding } from "@earendil-works/pi-tui";
import type { ChromeTier } from "./header.ts";
import { keyCap } from "./frame.ts";
import { pad, PANEL_BG, PANEL_MUTED, PANEL_TEXT, plainLabel, RESET } from "./panels-view.ts";

export type KeyContext = "composer" | "agents" | "agent" | "changes" | "task" | "lifecycle" | "proof" | "artifacts"
  | "inbox" | "usage" | "image" | "sidebar" | "queue" | "dialog" | "palette" | "keys" | "pager" | "board" | "choice" | "questions";
/**
 * One footer hint: a key cap and its label. A `lead` hint has no cap: it names the view that has the keys
 * (`keys → Changes · Tab to type`, short `→ Changes`) and a click on it replays Tab.
 */
export interface Hint {
  readonly id: string; readonly key: string; readonly label: string; readonly short?: string; readonly data?: string;
  readonly lead?: string;
}
export interface HintState {
  readonly context: KeyContext;
  readonly mode: { readonly taskBound: boolean; readonly answer: boolean; readonly lifecycle: boolean };
  /** True while Chat is the open view: its tool rows fold and unfold with Pi's tool-output key. */
  readonly chat?: boolean;
  readonly streaming: boolean;
  readonly queue: { readonly items: number; readonly editable: number; readonly control: boolean };
  /** Owner questions waiting in the dock; their card takes the queue's focus key when the queue has nothing to edit. */
  readonly questions?: number;
  /** The open view's name while it has the keys (`Changes`, `Task › Events`): the footer leads with it. */
  readonly lead?: string;
  /** AgentsAction ids offered for the selected row; in `board`, the BoardAction ids offered. */
  readonly selection?: readonly string[];
  readonly authors?: boolean; readonly legend?: boolean; readonly history?: boolean; readonly timelineSource?: "task" | "host";
  /** Usage only: `reread` while the collector applies (a bound task/run with a reader), `chart` while it has rows to draw. */
  readonly usage?: { readonly reread: boolean; readonly chart: boolean };
  /** Build sessions only: who Enter reaches, whether a child can take a message, a waiting board plan, docked options. */
  readonly build?: {
    readonly target?: { readonly id: string; readonly answer: boolean }; readonly steerable: boolean;
    readonly proposed: boolean; readonly choices: number;
    /** The docked dialog is the exit choice: Changes is one key away. */
    readonly exit?: boolean;
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
  "app.thinking.toggle": "ctrl+t", "app.model.cycleForward": "ctrl+p", "app.model.cycleBackward": "shift+ctrl+p",
  "app.clear": "ctrl+c", "app.exit": "ctrl+d", "tui.altScreen.pageUp": "pageUp", "tui.altScreen.pageDown": "pageDown",
  "tui.altScreen.top": "ctrl+home", "tui.altScreen.bottom": "ctrl+end",
};
/** First configured key via getKeybindings(); undefined when the user unbound it. */
export function piKey(id: string): string | undefined {
  const keys = getKeybindings();
  if (!keys.getDefinition(id as Keybinding)) return PI_DEFAULTS[id];
  return keys.getKeys(id as Keybinding)[0];
}
/** True when `data` is any key configured for the Pi binding `id` (its stock default when no binding table is installed). */
export function piPressed(data: string, id: string): boolean {
  const keys = getKeybindings();
  const fallback = PI_DEFAULTS[id];
  const bound: readonly string[] = keys.getDefinition(id as Keybinding) ? keys.getKeys(id as Keybinding) : fallback ? [fallback] : [];
  return bound.some(key => matchesKey(data, key as KeyId));
}
// Bindings scoped to Pi's own selectors, where panel keys are inert, never collide with a panel key.
const SCOPED = ["app.models.", "app.tree.", "tui.select."];
/** `key` unless the user bound it to an editor or app action. */
export function freeKey(key: KeyId): KeyId | undefined {
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
const VIEW_KEYS: readonly KeyId[] = ["alt+1", "alt+2", "alt+3", "alt+4", "alt+5", "alt+6"];
/** Alt+1 to Alt+6 open the main views in tab order, each unless the user bound it to an editor or app action. */
export function viewKey(index: number): KeyId | undefined {
  const key = VIEW_KEYS[index];
  return key === undefined ? undefined : freeKey(key);
}
/** The key that filters or searches the focused view (Changes paths, a job's output, the Inbox): `/` stays the composer's. */
export const FILTER_KEY: KeyId = "f3";

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
  search: hint("search", FILTER_KEY, "search"), background: letter("b", "background", "bg"),
};
const AGENT_ORDER = ["open", "answer", "transcript", "steer", "cancel", "model"];
const JOB_ORDER = ["output", "stop", "peek", "follow", "search", "background"];

function targetHint(): Hint | undefined {
  const key = targetKey();
  return key ? hint("target", key, "target", "to") : undefined;
}
function composerHints(state: HintState): (Hint | undefined)[] {
  const build = state.build, target = build?.target;
  // While the agent works, stop comes right after Enter so it survives every width.
  const stop = state.streaming ? piHint("app.interrupt", "interrupt", "stop") : undefined;
  // In Chat the tool rows' expand key comes last, so a narrow footer drops it first.
  const expand = state.chat ? piHint("app.tools.expand", "tools", "expand") : undefined;
  if (target) {
    // A child receives Enter: the queue and follow-up keys stay the parent's, so only Alt+T and stop remain.
    const id = plainLabel(target.id);
    const submit = piHint("tui.input.submit", "submit", `${target.answer ? "answer" : "steer"} ${id}`, target.answer ? "answer" : "steer");
    return [submit, target.answer ? undefined : stop, targetHint(), expand];
  }
  const to = build?.steerable ? targetHint() : undefined;
  // Pulling queued input back comes before the queue focus: it is the key the mock's composer footer names.
  const [dequeue, focus] = queueHints(state);
  return state.streaming
    ? [piHint("tui.input.submit", "submit", "steer"), stop, piHint("app.message.followUp", "followUp", "queue"), to, dequeue, focus, expand]
    : [piHint("tui.input.submit", "submit", "send"), to, dequeue, focus, expand];
}
function boardHints(state: HintState): (Hint | false | undefined)[] {
  // Without the view's offered actions, a task is assumed selected and the board reachable. Task keys need a task.
  const offered = new Set(state.selection ?? ["details", "refresh", ...(state.build?.proposed ? ["review"] : [])]);
  return [offered.has("details") && UP_DOWN("task", "task"), ...escAfterFirst([offered.has("review") && letter("p", "review plan", "plan"),
    offered.has("details") && ENTER("details"), offered.has("plan") && letter("n", "plan tasks", "plan"), offered.has("refresh") && letter("r", "refresh")], "chat")];
}
/** The view's actions with Esc right after the first one offered, so a narrow footer never drops the way out. */
function escAfterFirst(actions: readonly (Hint | false | undefined)[], back: string): (Hint | false | undefined)[] {
  const first = actions.findIndex(item => !!item);
  return first < 0 ? [ESC(back)] : [...actions.slice(0, first + 1), ESC(back), ...actions.slice(first + 1)];
}
function queueHints(state: HintState): (Hint | undefined)[] {
  const focus = queueFocusKey();
  // Without an editable item and queue control there is nothing the queue keys could do; waiting owner questions take the focus key.
  if (!state.queue.control || state.queue.editable < 1) return [undefined, focus && (state.questions ?? 0) > 0 ? hint("queue.focus", focus, "questions") : undefined];
  return [piHint("app.message.dequeue", "dequeue", "edit queued", "edit"), focus ? hint("queue.focus", focus, "queue") : undefined];
}
/** `keys → Changes · Tab to type`, short `→ Changes`: who has the keys and how to type; a click replays Tab. */
function leadHint(view: string): Hint {
  const name = plainLabel(view);
  return { id: "lead", key: "", label: `keys → ${name} · Tab to type`, short: `→ ${name}`, data: "\t", lead: name };
}

/** The contextual hints of section 5.4, in priority order, without F2 and F1; a focused view's lead comes first. */
export function contextHints(state: HintState): Hint[] {
  const hints = viewHints(state);
  return state.lead === undefined ? hints : [leadHint(state.lead), ...hints];
}
function viewHints(state: HintState): Hint[] {
  const pick = (...hints: (Hint | false | undefined)[]) => hints.filter((h): h is Hint => !!h);
  const legend = letter("?", state.legend ? "hide" : "legend");
  const filter = hint("filter", FILTER_KEY, "filter");
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
        filter, letter("c", "compare"), letter("o", "project"), letter("r", "refresh"), ESC("chat"));
    case "task":
      return pick(UP_DOWN("scroll", "scroll"), letter("r", "refresh"), state.mode.lifecycle && letter("g", "graph"),
        state.mode.lifecycle && letter("l", "events"), legend, ESC("chat"));
    case "lifecycle":
      return pick(J_K("select", "select"), ESC("task"), letter("r", "reconcile"),
        state.timelineSource === "task" && letter("h", "host"), state.timelineSource === "host" && letter("c", "canonical"), legend);
    case "proof":
      return pick(J_K("criterion", "criterion"), ENTER("open"), letter("a", "artifacts"), legend, letter("r", "refresh"), ESC("chat"));
    case "artifacts":
      return pick(J_K("select", "select"), ENTER("open"), ESC("proof"), letter("m", "mark"), letter("c", "compare"), letter("o", "browse"),
        letter("x", "export"), legend, letter("r", "refresh"));
    case "inbox":
      return pick(J_K("select", "select"), ENTER("inspect"), state.mode.answer && letter("a", "answer flow", "answer"),
        letter("h", state.history ? "pending" : "history"), filter, letter("r", "reread"), letter("?", "legend"), ESC("chat"));
    case "usage": {
      // Reread and token bars act on the collector; each shows only while the view says it can act. Absent means neither.
      const usage = state.usage;
      return pick(UP_DOWN("scroll", "scroll"),
        ...escAfterFirst([usage?.reread === true && letter("r", "reread"), usage?.chart === true && letter("c", "token bars", "bars"), legend], "chat"));
    }
    case "sidebar":
      return pick(pair("tab", "←→", "\x1b[C", "tab"), UP_DOWN("select", "select"), ENTER("open"), ESC("composer"));
    case "queue":
      return pick(UP_DOWN("select", "select"), ENTER("edit"), pair("move", "Shift+↑↓", "\x1b[1;2B", "move"), letter("x", "remove"), ESC("composer"));
    case "questions": {
      // Digits open a question directly; like the choice's pick, the hint names the range and replays nothing.
      const waiting = Math.min(3, state.questions ?? 0);
      return pick(UP_DOWN("select", "select"), ENTER("open"), ESC("composer"),
        waiting > 0 && { id: "pick", key: waiting > 1 ? `1-${waiting}` : "1", label: "open" });
    }
    case "dialog":
      // A Pi dialog or selector card (owner dialog, model, effort, settings) draws its own keys in the one hint row under
      // its card (owner-dialog-parts.ts `dialogHintRow`), and the panels' keys are inert while it is open: the footer repeats
      // none of them and draws no cap that could not act.
      return [];
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
      // Choose, then the way out, so a narrow footer never drops Esc. Digits pick directly; the hint names the
      // range and replays nothing, so a click never picks blindly.
      const choices = state.build?.choices ?? 0;
      return pick(ENTER("choose"), ESC("not now"), UP_DOWN("select", "select"), state.build?.exit && hint("f7", "f7", "changes"),
        choices > 0 && { id: "pick", key: choices > 1 ? `1-${Math.min(9, choices)}` : "1", label: "pick" });
    }
  }
}

/** Dialogs, the palette, the keys list and pagers own their keys: their footer has no F2 or F1 (both are inert while a dialog is open). */
export function footerBase(context: KeyContext): boolean {
  return context !== "dialog" && context !== "palette" && context !== "keys" && context !== "pager";
}

function hintWidth(item: Hint, short: boolean): number {
  // The lead has no cap, and its own two forms (full and short) are chosen apart from the other labels.
  if (item.lead !== undefined) return visibleWidth(item.label);
  const label = short ? item.short ?? item.label : item.label;
  return visibleWidth(item.key) + 2 + (label ? 1 + visibleWidth(label) : 0);
}
/** Hints that are never dropped for F2: the first one, and the way out (Esc, or stop) when it is among the first three. */
function essentials(hints: readonly Hint[]): number {
  const out = hints.slice(0, 3).findIndex(item => item.id === "escape" || item.id === "interrupt");
  return Math.min(hints.length, out < 0 ? 1 : out + 1);
}

/**
 * The footer row: a focused view's lead, the contextual hints, then `F2 actions` and `F1` when `base`. Every
 * candidate row that fits is scored, in this order: the essential hints (the first, and Esc or stop when it comes
 * right after the primary action) show; the lead shows; F2 shows; the lead reads in full (`Tab to type`); as many
 * contextual hints as possible show, in priority order; full labels rather than short ones. F1 reads `keys` when
 * nothing was dropped and `more` when something was; at worst it is a bare cap.
 */
export function footerRow(hints: readonly Hint[], tier: ChromeTier, width: number, base: boolean):
  { readonly line: string; readonly hits: readonly FooterHit[] } {
  const gap = tier === "tiny" ? 1 : 2;
  const room = Math.max(0, width - 1);
  const lead = hints[0]?.lead !== undefined ? hints[0] : undefined;
  const rest = lead ? hints.slice(1) : hints, needed = essentials(rest);
  const leads: (Hint | undefined)[] = lead ? [lead, { ...lead, label: lead.short ?? lead.label }, undefined] : [undefined];
  const f2 = base ? hint("f2", "f2", "actions") : undefined;
  const f1 = (label: string): Hint[] => base ? [hint("f1", "f1", label)] : [];
  const measure = (shown: readonly Hint[], short: boolean) =>
    shown.reduce((sum, item) => sum + hintWidth(item, short), 0) + gap * Math.max(0, shown.length - 1);
  const better = (score: readonly number[], than: readonly number[]) => {
    const at = score.findIndex((value, index) => value !== than[index]);
    return at >= 0 && score[at]! > than[at]!;
  };
  let best: { readonly shown: Hint[]; readonly short: boolean; readonly score: readonly number[] } | undefined;
  for (const short of [false, true]) for (const [form, shownLead] of leads.entries()) for (const withF2 of f2 ? [true, false] : [false]) {
    // Most hints first: the first that fits is the best row for this lead, F2 and label length.
    for (let count = rest.length; count >= 0; count--) {
      const shown = [...(shownLead ? [shownLead] : []), ...rest.slice(0, count), ...(withF2 && f2 ? [f2] : []),
        ...f1(count === rest.length && (withF2 || !f2) ? "keys" : "more")];
      if (measure(shown, short) > room) continue;
      const score = [count >= needed ? 1 : 0, shownLead ? 1 : 0, withF2 ? 1 : 0, form === 0 ? 1 : 0, count, short ? 0 : 1];
      if (!best || better(score, best.score)) best = { shown, short, score };
      break;
    }
  }
  const short = best?.short ?? false, shown = best?.shown ?? f1("");
  const hits: FooterHit[] = [];
  const parts: string[] = [];
  let x = 1;
  for (const item of shown) {
    const label = short && item.lead === undefined ? item.short ?? item.label : item.label;
    parts.push(item.lead !== undefined ? PANEL_MUTED + label.replace(item.lead, PANEL_TEXT + item.lead + PANEL_MUTED)
      : keyCap(item.key) + (label ? " " + PANEL_MUTED + label : ""));
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
 * `sidebarKey` is the chosen sidebar toggle; `questionsKey` the owner-question key when it is free.
 */
export function keyGroups(mode: {
  readonly taskBound: boolean; readonly answer: boolean; readonly jobs: boolean;
  readonly queueControl: boolean; readonly lifecycle: boolean; readonly build?: boolean;
  readonly sidebarKey?: KeyId; readonly questionsKey?: KeyId;
}): KeyGroup[] {
  const row = (binding: string, label: string): KeyRow => {
    const data = keyData(binding);
    return { key: keyLabel(binding), label, ...(data ? { data } : {}) };
  };
  const pi = (id: string, label: string): KeyRow | undefined => {
    const key = piKey(id);
    return key ? row(key, label) : undefined;
  };
  // A Pi key named for reference only: Enter in the list never replays it (Ctrl+C would clear the draft).
  const piInfo = (id: string, label: string): KeyRow | undefined => {
    const key = piKey(id);
    return key ? info(keyLabel(key), label) : undefined;
  };
  const info = (key: string, label: string): KeyRow => ({ key, label });
  const rows = (...items: (KeyRow | false | undefined)[]) => items.filter((item): item is KeyRow => !!item);
  const focus = mode.queueControl ? queueFocusKey() : undefined;
  const build = mode.build === true, target = build ? targetKey() : undefined;
  const taskBound = mode.taskBound && !build;
  // The views in tab order (header.ts mainTabs): the F-key, and Alt+n when the terminal and the user leave it free.
  const views = [
    taskBound && { key: "f5", label: "Task: the bound task, its plan and criteria" },
    { key: "f9", label: "Chat: the conversation" },
    { key: "f8", label: build ? "Agents: every agent, focus the list" : mode.jobs ? "Agents: helper calls and jobs, focus the list" : "Agents: helper calls, focus the list" },
    { key: "f7", label: build ? "Changes: files and diffs, filtered by agent" : "Changes: files and diffs in the chosen comparison" },
    taskBound && { key: "f10", label: "Proof: criteria, gate and review" },
    build && { key: "f10", label: "Board: tasks this session filed" },
    !build && { key: "f11", label: "Inbox: decisions waiting for the owner" },
  ].filter((item): item is { key: string; label: string } => !!item);
  const rail = mode.sidebarKey ?? "ctrl+b", filter = keyLabel(FILTER_KEY);
  const [pageUp, pageDown, top, bottom] = ["tui.altScreen.pageUp", "tui.altScreen.pageDown", "tui.altScreen.top", "tui.altScreen.bottom"].map(piKey);
  const groups: KeyGroup[] = [
    { title: "Views", rows: views.map((view, index) => {
      const alt = viewKey(index), base = row(view.key, view.label);
      return alt ? { ...base, key: `${base.key}/${keyLabel(alt)}` } : base;
    }) },
    { title: "Message", rows: rows(
      pi("tui.input.submit", build ? "Send; steer while something runs; steers the chosen agent when one receives" : "Send; steer while the agent works"),
      pi("app.message.followUp", build ? "Queue a follow-up for the parent" : "Queue as a follow-up"),
      target !== undefined && row(target, "Change the receiver: parent or one running agent"),
      pi("app.interrupt", "Stop the running turn"),
      mode.queueControl && pi("app.message.dequeue", "Pull the last queued input back to edit"),
      pi("app.model.select", "Choose the model"),
      pi("app.model.cycleForward", "Next model in the cycle"),
      pi("app.model.cycleBackward", "Previous model in the cycle"),
      pi("app.thinking.cycle", "Cycle the effort"),
      pi("app.thinking.toggle", "Show or hide thinking"),
      pi("app.tools.expand", "Expand or collapse tool output"),
      // Pi runs a draft that starts with `!` or `!!` in the owner's shell; the composer title says where its output goes.
      info("!cmd", build ? "Run cmd in your shell; its output goes to the parent (while the parent receives)" : "Run cmd in your shell; its output goes to the agent"),
      info("!!cmd", build ? "Run cmd in your shell; its output stays here (while the parent receives)" : "Run cmd in your shell; its output stays here"),
    ) },
    { title: "Scroll", rows: rows(
      pageUp !== undefined && pageDown !== undefined && info(`${keyLabel(pageUp)}/${keyLabel(pageDown)}`, "Scroll Chat or the open view a page"),
      top !== undefined && bottom !== undefined && info(`${keyLabel(top)}/${keyLabel(bottom)}`, "Chat: first message or latest message"),
      info("End", "Chat: latest message, when the draft is empty"),
      info("Wheel", "Scroll whatever is under the pointer"),
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
      mode.jobs && info(filter, "Search the job's output"),
      mode.jobs && info("b", "Move the job to the background"),
      info("esc", "Back to Chat; from a conversation, back to Agents"),
    ) },
    { title: "Changes", rows: rows(
      info("j/k", "Next or previous file"),
      info("n/p", "Next or previous hunk"),
      info("a", "Ask about the hunk (added to the draft, not sent)"),
      info(filter, "Filter paths"),
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
      info("?", mode.lifecycle ? "Show or hide the legend, here and in Graph and events" : "Show or hide the legend"),
    ) });
    groups.push({ title: "Proof", rows: rows(
      info("j/k", "Select a criterion"),
      info("⏎", "Open its detail"),
      info("a", "Artifacts"),
      info("?", "Show or hide the legend, here and in Artifacts"),
      info("r", "Refresh"),
    ) });
  }
  if (!build) {
    groups.push({ title: "Inbox", rows: rows(
      info("j/k", "Select an item"),
      info("⏎", "Inspect it"),
      mode.answer && info("a", "Answer flow instructions (nothing is sent)"),
      info("h", "History or pending"),
      info(filter, "Filter"),
      info("r", "Reread"),
      info("?", "Legend"),
    ) });
  }
  groups.push({ title: "Sidebar", rows: rows(
    row(rail, "Collapse to one row, or open it"),
    row("f6", "Focus the sidebar; again for the next tab"),
    info("←→", "Switch tab: Context, Files, Plan"),
    info("⏎", "Open the selected item"),
    info("Alt+,/Alt+.", "Narrow or widen the rail (from 105 columns)"),
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
  groups.push({ title: "Exit", rows: rows(
    build && info("/finish", "Choose what happens to this build branch"),
    info("/questions", "Answer owner questions the agent deferred"),
    piInfo("app.clear", "Clear the draft; twice in a row quits"),
    piInfo("app.exit", "Quit, when the draft is empty"),
  ) });
  groups.push({ title: "Anywhere", rows: rows(
    row("f2", "Search actions; unavailable ones say why"),
    info("F1", "This list; F1 or esc closes it"),
    info("Tab", "From a view: back to the composer to type"),
    mode.questionsKey && row(mode.questionsKey, "Answer waiting owner questions"),
    info("esc", "Close an expanded image; a click on it closes it too"),
  ) });
  return groups.filter(group => group.rows.length > 0);
}
