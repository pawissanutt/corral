// Settings (`/settings`, F2 Settings…): the owner's saved Corral settings on one main-area page, never a session
// control. STARTUP holds the model new chats and Build start with here (or Ask at launch); PERSONAL holds the Display
// defaults every workspace opens with. A value changes only by an explicit choice: Enter opens the setting's choices
// beside the list (under it when narrow) and Enter there saves; nothing cycles. F3 searches, Esc steps back: the
// search, then the choices, then the page. What a save writes is the host's (`SettingsControl`); a saved personal
// value also applies to this terminal at once, while F2 Display stays this session's only.
import type { SelectorComponent } from "@earendil-works/pi-coding-agent";
import { decodeKittyPrintable, getKeybindings, matchesKey, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { TuiMouseEvent, TuiMouseEventResult } from "@earendil-works/pi-tui";
import type { Role } from "../roles/types.ts";
import type { Appearance, Bell, Density, Motion } from "./appearance.ts";
import { FILTER_KEY, keyData, keyLabel, piKey } from "./keymap.ts";
import { dialogHintRow, type DialogHint, type DialogHintHit } from "./owner-dialog-parts.ts";
import { backLine, bold, listRow, wrap } from "./panel-parts.ts";
import { pad, plainLabel, PANEL_BLUE, PANEL_ERROR, PANEL_LINE, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";

/** The Display defaults saved for the owner across workspaces; F2 Display changes only this terminal's. */
export interface PersonalSettings {
  readonly appearance: Appearance;
  readonly density: Density;
  readonly motion: Motion;
  readonly bell: Bell;
}
/** One model a new chat or Build can start with, as the host lists it. */
export interface StartupModelOption {
  /** The value saved, `provider/id`. */
  readonly selector: string;
  readonly provider: string;
  readonly id: string;
  /** The provider and billing route it runs on (`Anthropic · Claude subscription`). */
  readonly label: string;
}
/** The startup model this page can change, or why it cannot here. */
export type WorkspaceSettings =
  | {
    readonly editable: true;
    /** Where it is saved: the workspace's name, or the host's name for a session outside any workspace. */
    readonly name: string;
    /** `null`: Ask at launch. */
    readonly startupModel: string | null;
    /** Why the saved model cannot start a session now; a saved model no listed option offers is flagged too. */
    readonly unavailable?: string;
  }
  | { readonly editable: false; readonly reason: string };
export interface SettingsSnapshot {
  readonly personal: PersonalSettings;
  readonly workspace: WorkspaceSettings;
}
/** The host's durable settings. Each write rejects with an owner-readable Error and then wrote nothing. */
export interface SettingsControl {
  read(): Promise<SettingsSnapshot>;
  listStartupModels(): Promise<readonly StartupModelOption[]>;
  /** `null`: Ask at launch. */
  setStartupModel(selector: string | null): Promise<void>;
  /** One changed field per call. */
  setPersonal(patch: Partial<PersonalSettings>): Promise<void>;
}

type PersonalId = keyof PersonalSettings;
type SettingId = "startup" | PersonalId;
interface Choice {
  readonly value: string;
  readonly label: string;
  /** Muted beside the label: a billing route, what a value does. */
  readonly note?: string;
  /** Shown, never chosen: why. */
  readonly disabled?: string;
}
interface PersonalSpec {
  readonly id: PersonalId;
  readonly label: string;
  readonly describe: string;
  readonly choices: readonly Choice[];
}
const PERSONAL: readonly PersonalSpec[] = [
  { id: "appearance", label: "Appearance", describe: "The palette Corral Pi terminals open with.", choices: [
    { value: "dark", label: "Dark" }, { value: "light", label: "Light" }, { value: "contrast", label: "Contrast", note: "highest contrast" },
  ] },
  { id: "density", label: "Density", describe: "Spacing around the composer's draft.", choices: [
    { value: "comfortable", label: "Comfortable" }, { value: "compact", label: "Compact", note: "one cell less" },
  ] },
  { id: "motion", label: "Motion", describe: "Whether spinners animate; still shows a steady ● and times keep advancing.", choices: [
    { value: "animate", label: "Animate" }, { value: "still", label: "Still", note: "no animation" },
  ] },
  { id: "bell", label: "Attention bell", describe: "Ring the terminal bell once when a session starts needing you.", choices: [
    { value: "off", label: "Off" }, { value: "on", label: "On" },
  ] },
];
/** The Ask at launch choice's value; every model's is its selector. */
const ASK = "";
/** Main-view cells from which the detail stands beside the list. */
const WIDE = 84;
const LIST_MIN = 34, LIST_MAX = 50;
/** The detail's label column. */
const LABEL = 13;
/** Fewest choice rows the choices keep, however short the view. */
const CHOICES_MIN = 3;

type Load<T> = { readonly state: "loading" } | { readonly state: "ready"; readonly value: T } | { readonly state: "error"; readonly error: string };
interface Status { readonly tone: "muted" | "success" | "warning" | "error"; readonly text: string }
interface Row { readonly id: SettingId; readonly label: string; readonly value: string; readonly color: string }
type Hit = { readonly kind: "back" } | { readonly kind: "row"; readonly id: SettingId } | { readonly kind: "choice"; readonly index: number };
interface Span { readonly from: number; readonly to: number; readonly hit: Hit }

const muted = (text: string) => PANEL_MUTED + text + PANEL_TEXT;
const message = (error: unknown) => plainLabel(error instanceof Error ? error.message : String(error)) || "unknown error";
const TONE: Readonly<Record<Status["tone"], () => string>> = {
  muted: () => PANEL_MUTED, success: () => PANEL_SUCCESS, warning: () => PANEL_WARNING, error: () => PANEL_ERROR,
};

/** A group heading in muted bold, its tail as written, then a rule in the line colour to `width`. */
function groupRule(head: string, tail: string, width: number): string {
  const text = bold(head) + (tail ? ` · ${plainLabel(tail)}` : "");
  const label = visibleWidth(text) > Math.max(1, width - 2) ? truncateToWidth(text, Math.max(1, width - 2), "…") : text;
  return `${PANEL_MUTED}${label} ${PANEL_LINE}${"─".repeat(Math.max(0, width - visibleWidth(label) - 1))}${PANEL_TEXT}`;
}
/** A muted label column beside `value` wrapped in `color`. */
function fact(label: string, value: string, width: number, color = PANEL_TEXT): string[] {
  return wrap(plainLabel(value) || "—", Math.max(1, width - LABEL - 1))
    .map((row, at) => `${PANEL_MUTED}${pad(at ? "" : label, LABEL)}${PANEL_TEXT} ${color}${row}${PANEL_TEXT}`);
}
function patchOf(id: PersonalId, value: string): Partial<PersonalSettings> {
  switch (id) {
    case "appearance": return { appearance: value as Appearance };
    case "density": return { density: value as Density };
    case "motion": return { motion: value as Motion };
    case "bell": return { bell: value as Bell };
  }
}
const choiceLabel = (spec: PersonalSpec, value: string) => spec.choices.find(choice => choice.value === value)?.label ?? plainLabel(value);
const matches = (query: string, ...texts: readonly string[]) => texts.some(text => text.toLowerCase().includes(query.toLowerCase()));

export interface SettingsPageOptions {
  readonly control: SettingsControl;
  /** Worker and reviewer sessions never change the startup model, whatever the host offers. */
  readonly role: Role;
  /** This terminal's Display values now (F2 may have changed them for this session). */
  readonly session: () => PersonalSettings;
  /** A personal value was saved: this terminal takes it at once. */
  readonly apply: (patch: Partial<PersonalSettings>) => void;
  readonly changed: () => void;
}

/** The page's state and drawing; the workbench lays it out as a main view, `settingsKeys` holds Pi's focus for it. */
export class SettingsPage {
  private snapshot: Load<SettingsSnapshot> = { state: "loading" };
  private models: Load<readonly StartupModelOption[]> | undefined;
  private selected: SettingId = "startup";
  /** The open choices: whose, and the highlighted index in `choices(id)`. */
  private choosing: { readonly id: SettingId; index: number } | undefined;
  /** The F3 search while it is open; it filters the choices while they are open, else the settings. */
  private query: string | undefined;
  private status: Partial<Record<SettingId, Status>> = {};
  private saving: SettingId | undefined;
  /** Each open starts a generation; reads and saves of an older one leave the page alone. */
  private generation = 0;
  private close: (() => void) | undefined;
  private hits = new Map<number, readonly Span[]>();

  constructor(private readonly options: SettingsPageOptions) {}

  get isOpen(): boolean { return this.close !== undefined; }

  /** Opens afresh: the saved settings are read again; `close` leaves the page (Esc at its top, the back line). */
  open(close: () => void): void {
    this.close = close;
    const generation = ++this.generation;
    this.snapshot = { state: "loading" }; this.models = undefined;
    this.selected = "startup"; this.choosing = undefined; this.query = undefined; this.status = {}; this.saving = undefined;
    this.options.control.read().then(snapshot => {
      if (generation !== this.generation) return;
      this.snapshot = { state: "ready", value: snapshot };
      if (!this.editable()) this.selected = "appearance";
      if (this.editable()) this.loadModels(generation);
      this.options.changed();
    }, error => {
      if (generation !== this.generation) return;
      this.snapshot = { state: "error", error: message(error) };
      this.options.changed();
    });
  }

  /** The page was left; pending reads change nothing, a pending save still applies what it saved. */
  closed(): void {
    this.close = undefined;
    this.generation++;
  }

  private loadModels(generation: number): void {
    this.models = { state: "loading" };
    this.options.control.listStartupModels().then(models => {
      if (generation !== this.generation) return;
      this.models = { state: "ready", value: models };
      this.options.changed();
    }, error => {
      if (generation !== this.generation) return;
      this.models = { state: "error", error: message(error) };
      this.options.changed();
    });
  }

  private ready(): SettingsSnapshot | undefined {
    return this.snapshot.state === "ready" ? this.snapshot.value : undefined;
  }

  /** The startup model this session may change, or undefined with `lockedReason()` saying why. */
  private editable(): Extract<WorkspaceSettings, { editable: true }> | undefined {
    const workspace = this.ready()?.workspace;
    if (!workspace?.editable || this.options.role === "worker" || this.options.role === "reviewer") return undefined;
    return workspace;
  }

  private lockedReason(): string {
    const role = this.options.role, workspace = this.ready()?.workspace;
    if (role === "worker" || role === "reviewer") return `A ${role} session does not change startup settings; change them from a chat or Build session.`;
    return workspace && !workspace.editable ? plainLabel(workspace.reason) : "Not available here.";
  }

  /** Why the saved startup model cannot start a session now, if it cannot. */
  private unavailable(): string | undefined {
    const workspace = this.editable();
    if (!workspace || workspace.startupModel === null) return undefined;
    if (workspace.unavailable) return plainLabel(workspace.unavailable);
    const models = this.models;
    if (models?.state === "ready" && !models.value.some(model => model.selector === workspace.startupModel)) {
      return "No signed-in provider offers it now; sign in again or choose another.";
    }
    return undefined;
  }

  private savedModel(): StartupModelOption | undefined {
    const workspace = this.editable(), models = this.models;
    return workspace && models?.state === "ready" ? models.value.find(model => model.selector === workspace.startupModel) : undefined;
  }

  private rows(): Row[] {
    const snapshot = this.ready();
    if (!snapshot) return [];
    const workspace = this.editable(), saving = (id: SettingId) => this.saving === id ? " · saving…" : "";
    let startup: Row;
    if (!workspace) startup = { id: "startup", label: "Startup model", value: "not here", color: PANEL_MUTED };
    else {
      const unavailable = this.unavailable() !== undefined;
      startup = { id: "startup", label: "Startup model", color: unavailable ? PANEL_WARNING : PANEL_BLUE,
        value: (workspace.startupModel === null ? "Ask at launch" : plainLabel(workspace.startupModel)) + (unavailable ? " · unavailable" : "") + saving("startup") };
    }
    return [startup, ...PERSONAL.map(spec => ({
      id: spec.id, label: spec.label, color: PANEL_BLUE, value: choiceLabel(spec, snapshot.personal[spec.id]) + saving(spec.id),
    }))];
  }

  /** The settings the search keeps, in page order. */
  private shownRows(): Row[] {
    const query = this.choosing ? "" : this.query ?? "";
    const heading = (id: SettingId) => id === "startup" ? `startup ${this.editable()?.name ?? ""}` : "personal across workspaces";
    return this.rows().filter(row => !query || matches(query, row.label, row.value, heading(row.id)));
  }

  private choices(id: SettingId): Choice[] {
    if (id !== "startup") return [...(PERSONAL.find(spec => spec.id === id)?.choices ?? [])];
    const workspace = this.editable();
    if (!workspace) return [];
    const models = this.models?.state === "ready" ? this.models.value : [];
    const list: Choice[] = [
      { value: ASK, label: "Ask at launch", note: "choose each time" },
      ...models.map(model => ({ value: model.selector, label: `${plainLabel(model.provider)}/${plainLabel(model.id)}`, note: plainLabel(model.label) })),
    ];
    const saved = workspace.startupModel, reason = this.unavailable();
    if (saved !== null && reason && !list.some(choice => choice.value === saved)) {
      list.push({ value: saved, label: plainLabel(saved), note: "unavailable", disabled: reason });
    }
    return list;
  }

  private current(id: SettingId): string | undefined {
    const snapshot = this.ready();
    if (!snapshot) return undefined;
    if (id === "startup") { const workspace = this.editable(); return workspace ? workspace.startupModel ?? ASK : undefined; }
    return snapshot.personal[id];
  }

  /** Choice indexes the search keeps, in order. */
  private shownChoices(): number[] {
    const choosing = this.choosing;
    if (!choosing) return [];
    const query = this.query ?? "";
    return this.choices(choosing.id).flatMap((choice, index) => !query || matches(query, choice.label, choice.note ?? "") ? [index] : []);
  }

  /** The keys the page offers now, for the hint row in the composer's place. */
  hints(): DialogHint[] {
    const hint = (binding: string, label: string, key = keyLabel(binding)): DialogHint[] => {
      const data = keyData(binding);
      return data ? [{ key, label, data }] : [];
    };
    const confirm = piKey("tui.select.confirm") ?? "enter", cancel = piKey("tui.select.cancel") ?? "escape";
    const up = piKey("tui.select.up") ?? "up", down = piKey("tui.select.down") ?? "down";
    const choosing = this.choosing !== undefined;
    return [
      ...hint(confirm, choosing ? "save" : "choose"),
      ...hint(cancel, this.query !== undefined ? "clear search" : choosing ? "back" : "close"),
      ...hint(down, "select", up === "up" && down === "down" ? "↑↓" : `${keyLabel(up)}/${keyLabel(down)}`),
      ...(this.query === undefined ? hint(FILTER_KEY, "search") : []),
    ];
  }

  handleInput(data: string): void {
    if (!this.close) return;
    this.key(data);
    this.options.changed();
  }

  private key(data: string): void {
    const keys = getKeybindings();
    if (keys.matches(data, "tui.select.cancel") || matchesKey(data, "escape")) { this.back(); return; }
    if (keys.matches(data, "tui.select.confirm") || data === "\r" || data === "\n") { this.enter(); return; }
    if (keys.matches(data, "tui.select.up")) { this.move(-1); return; }
    if (keys.matches(data, "tui.select.down")) { this.move(1); return; }
    if (keys.matches(data, "tui.select.pageUp")) { this.move(-5); return; }
    if (keys.matches(data, "tui.select.pageDown")) { this.move(5); return; }
    if (matchesKey(data, FILTER_KEY)) { this.query ??= ""; return; }
    if (this.query === undefined) return; // Without the search, typing changes nothing.
    if (matchesKey(data, "backspace")) { this.search(this.query.slice(0, -1)); return; }
    const text = decodeKittyPrintable(data) ?? data;
    if (text && !text.startsWith("\x1b") && !/[\u0000-\u001f\u007f-\u009f]/.test(text)) this.search(this.query + text);
  }

  /** Esc: the search closes, else the choices, else the page. */
  private back(): void {
    if (this.query !== undefined) { this.search(undefined); return; }
    if (this.choosing) { this.choosing = undefined; return; }
    const close = this.close;
    this.close = undefined;
    close?.();
  }

  private search(query: string | undefined): void {
    this.query = query;
    // The selection stays on what the search keeps.
    if (this.choosing) {
      const shown = this.shownChoices();
      if (shown.length && !shown.includes(this.choosing.index)) this.choosing.index = shown[0]!;
    } else {
      const shown = this.shownRows();
      if (shown.length && !shown.some(row => row.id === this.selected)) this.selected = shown[0]!.id;
    }
  }

  private move(delta: number): void {
    if (this.choosing) {
      const shown = this.shownChoices(), at = shown.indexOf(this.choosing.index);
      const next = shown[Math.max(0, Math.min(shown.length - 1, (at < 0 ? 0 : at) + delta))];
      if (next !== undefined) this.choosing.index = next;
      return;
    }
    const shown = this.shownRows(), at = shown.findIndex(row => row.id === this.selected);
    const next = shown[Math.max(0, Math.min(shown.length - 1, (at < 0 ? 0 : at) + delta))];
    if (next) this.selected = next.id;
  }

  /** Enter: the selected setting's choices open; with them open, the highlighted one is saved. */
  private enter(): void {
    if (this.choosing) { this.choose(); return; }
    if (!this.ready() || !this.shownRows().some(row => row.id === this.selected)) return;
    this.openChoices(this.selected);
  }

  private openChoices(id: SettingId): void {
    if (id === "startup" && !this.editable()) { this.status.startup = { tone: "warning", text: this.lockedReason() }; return; }
    const current = this.current(id), index = this.choices(id).findIndex(choice => choice.value === current);
    this.selected = id; this.query = undefined;
    this.choosing = { id, index: Math.max(0, index) };
  }

  private choose(): void {
    const choosing = this.choosing;
    if (!choosing) return;
    if (!this.shownChoices().includes(choosing.index)) return;
    const id = choosing.id, choice = this.choices(id)[choosing.index];
    if (!choice) return;
    if (choice.disabled) { this.status[id] = { tone: "warning", text: choice.disabled }; return; }
    if (this.saving !== undefined) { this.status[id] = { tone: "muted", text: "Wait for the save in progress." }; return; }
    this.choosing = undefined; this.query = undefined;
    if (choice.value === this.current(id)) { this.status[id] = { tone: "muted", text: `${choice.label} is already saved.` }; return; }
    if (id === "startup") this.saveStartup(choice);
    else this.savePersonal(id, choice);
  }

  private saveStartup(choice: Choice): void {
    const workspace = this.editable(), generation = this.generation;
    if (!workspace) return;
    const selector = choice.value === ASK ? null : choice.value;
    this.saving = "startup"; this.status.startup = { tone: "muted", text: "Saving…" };
    this.options.control.setStartupModel(selector).then(() => {
      if (generation !== this.generation) return;
      const snapshot = this.ready();
      // The saved model was just offered, so it is available until the next read says otherwise.
      if (snapshot) this.snapshot = { state: "ready", value: { ...snapshot, workspace: { editable: true, name: workspace.name, startupModel: selector } } };
      this.status.startup = { tone: "success", text: selector === null
        ? "Saved · new chats and Build here ask for a model at launch."
        : `Saved · new chats and Build here start with ${choice.label}.` };
    }, error => {
      if (generation !== this.generation) return;
      this.status.startup = { tone: "error", text: `Not saved · ${message(error)}` };
    }).finally(() => {
      if (generation !== this.generation) return;
      this.saving = undefined;
      this.options.changed();
    });
  }

  private savePersonal(id: PersonalId, choice: Choice): void {
    const patch = patchOf(id, choice.value), generation = this.generation;
    this.saving = id; this.status[id] = { tone: "muted", text: "Saving…" };
    this.options.control.setPersonal(patch).then(() => {
      // Saved: this terminal takes it whether or not the page is still open.
      this.options.apply(patch);
      if (generation !== this.generation) return;
      const snapshot = this.ready();
      if (snapshot) this.snapshot = { state: "ready", value: { ...snapshot, personal: { ...snapshot.personal, ...patch } } };
      this.status[id] = { tone: "success", text: `Saved · ${choice.label} in every workspace, and in this terminal now.` };
    }, error => {
      if (generation !== this.generation) return;
      this.status[id] = { tone: "error", text: `Not saved · ${message(error)}` };
    }).finally(() => {
      if (generation !== this.generation) return;
      this.saving = undefined;
      this.options.changed();
    });
  }

  /**
   * The page at `width` cells, fitted to `rows` rows when the layout knows them (0: not yet): the back line to `back`,
   * the search row while it is open, the list, and the selected setting's detail and choices beside it from WIDE cells,
   * under it below.
   */
  render(width: number, rows: number, back: string): string[] {
    const w = Math.max(1, width), lines: string[] = [], hits = new Map<number, Span[]>();
    const add = (row: number, from: number, to: number, hit: Hit) => hits.set(row, [...(hits.get(row) ?? []), { from, to, hit }]);
    const head = backLine(back, "Settings", "saved · F2 Display is this terminal only", w);
    add(0, 0, head.to, { kind: "back" });
    lines.push(head.line);
    if (this.query !== undefined) lines.push(this.searchRow(w));
    lines.push("");
    const budget = rows > 0 ? rows - lines.length : Number.POSITIVE_INFINITY;
    if (this.snapshot.state !== "ready") {
      lines.push(this.snapshot.state === "loading" ? muted("Reading saved settings…")
        : `${PANEL_ERROR}Settings unavailable · ${this.snapshot.error}${PANEL_TEXT}`);
      this.hits = hits;
      return lines.map(line => truncateToWidth(line, w, "…"));
    }
    const wide = w >= WIDE;
    const listWidth = wide ? Math.max(LIST_MIN, Math.min(LIST_MAX, Math.floor(w * 0.42))) : w;
    const detailWidth = wide ? Math.max(1, w - listWidth - 3) : w;
    const list = this.listLines(listWidth);
    const detail = this.detailLines(detailWidth, wide ? budget : budget - list.lines.length - 1);
    const top = lines.length;
    if (wide) {
      const height = Math.max(list.lines.length, detail.lines.length), from = listWidth + 3;
      for (let row = 0; row < height; row++) {
        lines.push(pad(list.lines[row] ?? "", listWidth) + ` ${PANEL_LINE}│${PANEL_TEXT} ` + (detail.lines[row] ?? ""));
        const id = list.at.get(row), index = detail.at.get(row);
        if (id !== undefined) add(top + row, 0, listWidth, { kind: "row", id });
        if (index !== undefined) add(top + row, from, w, { kind: "choice", index });
      }
    } else {
      list.lines.forEach((line, row) => {
        lines.push(line);
        const id = list.at.get(row);
        if (id !== undefined) add(top + row, 0, w, { kind: "row", id });
      });
      lines.push("");
      const below = lines.length;
      detail.lines.forEach((line, row) => {
        lines.push(line);
        const index = detail.at.get(row);
        if (index !== undefined) add(below + row, 0, w, { kind: "choice", index });
      });
    }
    this.hits = hits;
    return lines.map(line => truncateToWidth(line, w, "…"));
  }

  private searchRow(width: number): string {
    const query = this.query ?? "", count = this.choosing ? `${this.shownChoices().length} / ${this.choices(this.choosing.id).length}`
      : `${this.shownRows().length} / ${this.rows().length}`;
    const lead = `${PANEL_BLUE}search ›${PANEL_TEXT} `, room = Math.max(1, width - visibleWidth(lead) - visibleWidth(count) - 3);
    const text = query ? truncateToWidth(plainLabel(query), room - 1, "…") : "";
    // A block marks where typing goes; the search is the only typing the page takes.
    const field = text + "\x1b[7m \x1b[27m" + (query ? "" : muted(this.choosing ? " type to filter the choices" : " type to filter the settings"));
    return pad(lead + field, width - visibleWidth(count) - 1) + " " + muted(count);
  }

  private listLines(width: number): { readonly lines: string[]; readonly at: Map<number, SettingId> } {
    const lines: string[] = [], at = new Map<number, SettingId>(), shown = this.shownRows(), focused = this.choosing === undefined;
    const workspace = this.editable();
    const groups: readonly { readonly head: string; readonly tail: string; readonly rows: readonly Row[] }[] = [
      { head: "STARTUP", tail: workspace?.name ?? "", rows: shown.filter(row => row.id === "startup") },
      { head: "PERSONAL", tail: "across workspaces", rows: shown.filter(row => row.id !== "startup") },
    ];
    for (const group of groups) {
      if (!group.rows.length) continue;
      if (lines.length) lines.push("");
      lines.push(groupRule(group.head, group.tail, width));
      for (const row of group.rows) {
        at.set(lines.length, row.id);
        const selected = row.id === this.selected;
        lines.push(listRow(selected ? bold(row.label) : row.label, row.color + row.value + PANEL_TEXT, selected, width, focused));
      }
    }
    if (!lines.length) lines.push(muted(`No setting matches “${plainLabel(this.query ?? "")}”`));
    return { lines, at };
  }

  /** The selected setting: what is saved, what it does, the last save's outcome, and its choices while open. */
  private detailLines(width: number, budget: number): { readonly lines: string[]; readonly at: Map<number, number> } {
    const lines: string[] = [], at = new Map<number, number>(), snapshot = this.ready(), id = this.selected;
    if (!snapshot) return { lines, at };
    if (id === "startup") {
      lines.push(groupRule("STARTUP MODEL", "", width));
      const workspace = this.editable();
      if (!workspace) lines.push(...wrap(muted(this.lockedReason()), width));
      else {
        const saved = workspace.startupModel, model = this.savedModel(), reason = this.unavailable();
        lines.push(...fact("saved", saved === null ? "Ask at launch" : saved, width, PANEL_BLUE));
        if (saved !== null) {
          if (model) {
            lines.push(...fact("model", `${model.provider}/${model.id}`, width));
            lines.push(...fact("billing", model.label, width));
          } else if (this.models?.state === "loading") lines.push(...fact("billing", "checking signed-in providers…", width, PANEL_MUTED));
          else if (this.models?.state === "error") lines.push(...fact("billing", `unknown · ${this.models.error}`, width, PANEL_WARNING));
        }
        if (reason) lines.push(...fact("status", `unavailable · ${reason}`, width, PANEL_WARNING));
        lines.push(...fact("applies to", `New chats and Build in ${workspace.name}. Running sessions keep their model.`, width));
        if (saved === null) lines.push(...fact("at launch", "Each new chat or Build asks which model to start with.", width));
      }
    } else {
      const spec = PERSONAL.find(item => item.id === id)!, saved = snapshot.personal[id], here = this.options.session()[id];
      lines.push(groupRule(spec.label.toUpperCase(), "", width));
      lines.push(...fact("saved", choiceLabel(spec, saved), width, PANEL_BLUE));
      lines.push(...fact("this terminal", here === saved ? choiceLabel(spec, here) : `${choiceLabel(spec, here)} · F2, this session only`, width));
      lines.push(...fact("applies to", "Every workspace. Saving also applies it to this terminal now.", width));
      lines.push(...wrap(muted(spec.describe), width));
    }
    const status = this.status[id];
    if (status) lines.push(...wrap(TONE[status.tone]() + status.text + PANEL_TEXT, width));
    const choosing = this.choosing;
    if (!choosing || choosing.id !== id) {
      if (id !== "startup" || this.editable()) lines.push("", muted("Enter shows the choices; nothing changes until you pick one."));
      return { lines, at };
    }
    lines.push("", groupRule("CHOOSE", "", width));
    const choices = this.choices(id), shown = this.shownChoices();
    if (id === "startup" && this.models?.state === "loading") lines.push(muted("Reading signed-in models…"));
    if (id === "startup" && this.models?.state === "error") lines.push(`${PANEL_WARNING}Models unavailable · ${this.models.error}${PANEL_TEXT}`);
    if (!shown.length) { lines.push(muted(this.query ? `No choice matches “${plainLabel(this.query)}”` : "Nothing to choose")); return { lines, at }; }
    // The choices fit the rows left, at least CHOICES_MIN, scrolled to keep the highlighted one; markers say what is hidden.
    // The highlighted choice's note in full under the choices when its row clips it (a long billing route).
    const noteRoom = Math.max(0, Math.floor(width * 0.45)), note = choices[choosing.index]?.note ?? "";
    const full = note && visibleWidth(note) > noteRoom ? wrap(muted(note), Math.max(1, width - 4)).map(row => "    " + row) : [];
    const room = Number.isFinite(budget) ? Math.max(CHOICES_MIN, budget - lines.length - full.length) : shown.length;
    let first = 0, count = shown.length;
    if (shown.length > room) {
      count = Math.max(1, room - 2);
      const at = Math.max(0, shown.indexOf(choosing.index));
      first = Math.max(0, Math.min(shown.length - count, at - Math.floor(count / 2)));
    }
    if (first > 0) lines.push(muted(`  ↑ ${first} more`));
    const current = this.current(id);
    for (const index of shown.slice(first, first + count)) {
      const choice = choices[index]!, highlighted = index === choosing.index;
      const mark = choice.value === current ? `${PANEL_BLUE}●${PANEL_TEXT}` : muted("○");
      const label = choice.disabled ? muted(choice.label) : highlighted ? bold(choice.label) : choice.label;
      const note = choice.note && noteRoom >= 8 ? muted(truncateToWidth(choice.note, noteRoom, "…")) : "";
      at.set(lines.length, index);
      lines.push(listRow(`${mark} ${label}`, note, highlighted, width, true));
    }
    const rest = shown.length - first - count;
    if (rest > 0) lines.push(muted(`  ↓ ${rest} more`));
    lines.push(...full);
    return { lines, at };
  }

  /** A click: the back line leaves; a setting opens its choices; a choice is highlighted, and saved when clicked again. */
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (!this.close || event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl) return undefined;
    const hit = this.hits.get(event.y)?.find(span => event.x >= span.from && event.x < span.to)?.hit;
    if (!hit) return { handled: true };
    if (hit.kind === "back") { this.query = undefined; this.choosing = undefined; this.back(); }
    else if (hit.kind === "row") { this.selected = hit.id; this.choosing = undefined; this.openChoices(hit.id); }
    else if (this.choosing) {
      if (this.choosing.index === hit.index) this.choose();
      else this.choosing.index = hit.index;
    }
    this.options.changed();
    return { handled: true };
  }
}

/**
 * What holds Pi's focus in the composer's place while the page shows: one row of the page's keys (a click replays
 * one), every key to the page. `left` runs once when Pi closes it, however it closes.
 */
class SettingsKeys implements SelectorComponent {
  focused = false;
  private hits: readonly DialogHintHit[] = [];
  private live = true;

  constructor(private readonly page: SettingsPage, private readonly left: () => void) {}

  render(width: number): readonly string[] {
    const row = dialogHintRow(this.page.hints(), width);
    this.hits = row.hits;
    return [row.line];
  }

  handleInput(data: string): void {
    if (this.live) this.page.handleInput(data);
  }

  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (!this.live) return { handled: true };
    if (event.type === "click" && event.button === "left" && event.y === 0) {
      const hit = this.hits.find(item => event.x >= item.from && event.x < item.to);
      if (hit) this.page.handleInput(hit.data);
    }
    return { handled: true };
  }

  invalidate(): void {}

  dispose(): void {
    if (!this.live) return;
    this.live = false;
    this.left();
  }
}

/** The component Pi shows in the composer's place for the open page; `left` runs once as Pi closes it. */
export function settingsKeys(page: SettingsPage, left: () => void): SelectorComponent {
  return new SettingsKeys(page, left);
}
