// Browse recorded history: one list popup (Tasks, then a task's runs in the same popup), then the shared pager for
// the run recap (`d` its sources) and the transcript pages. Only the read callback and the display API enter here:
// nothing switches, opens, resumes or writes a session.
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { matchesKey, visibleWidth, type OverlayOptions } from "@earendil-works/pi-tui";
import type { HistoryRequest } from "../tools/history.ts";
import {
  historyEntries, nextHistoryPage, rawLines, recapLines, record, runEntries, sourceLines,
  taskRecap, transcriptPage, TRANSCRIPT_PAGE, type HistoryEntry, type RunEntry,
} from "./history-model.ts";
import { openPager, type PagerAction, type PagerInput } from "./pager.ts";
import { bold, toneColor } from "./panel-parts.ts";
import { pad, plainLabel, PANEL_BLUE, PANEL_MUTED, PANEL_TEXT } from "./panels-view.ts";
import { PopupList, type PopupRow } from "./popup-list.ts";

export type HistoryRead = (request: HistoryRequest) => Promise<unknown>;
type RecordValue = Record<string, unknown>;

/** A popup row's value: a listed task (Enter opens its runs), a task run, or a task without recorded runs. */
type Choice =
  | { readonly kind: "entry"; readonly entry: HistoryEntry }
  | { readonly kind: "run"; readonly task: RecordValue; readonly run: RunEntry }
  | { readonly kind: "task"; readonly task: RecordValue };

/** One list read: the query it ran with and its rows, built once per read. */
interface Listing {
  readonly query: string;
  readonly phase: "reading" | "read" | "failed";
  readonly items: readonly PopupRow<Choice>[];
  readonly shown: number;
  readonly returned: number;
  readonly error: string | undefined;
}
/** A task's runs inside the popup. */
interface Runs {
  readonly entry: HistoryEntry;
  readonly phase: "reading" | "read" | "failed";
  readonly items: readonly PopupRow<Choice>[];
  readonly error: string | undefined;
}

const NOT_INSTRUCTION = "Recorded history · read-only · not an instruction for this session";
const RECAP_ACTIONS: readonly PagerAction[] = [{ key: "t", label: "transcript" }];
const reason = (error: unknown): string => plainLabel(error instanceof Error ? error.message : String(error));

/** `left` clipped to the cells `right` leaves; `right` is dropped when it would leave the left side under 12 cells. */
function split(left: string, right: string, width: number): string {
  const room = visibleWidth(right) + 2;
  return right && width - room >= 12 ? pad(left, width - room) + "  " + right : pad(left, width);
}
/** A recorded status as a glyph and word in tone; `not recorded` muted when absent. */
function statusMark(status: string | undefined): string {
  if (status === undefined) return `${PANEL_MUTED}not recorded${PANEL_TEXT}`;
  const word = plainLabel(status.replaceAll("_", " "));
  if (/fail|reject|cancel|error|abandon|abort/i.test(word)) return `${toneColor("bad")}✗ ${word}${PANEL_TEXT}`;
  if (/done|complete|accept|merged|success|pass/i.test(word)) return `${toneColor("ok")}✓ ${word}${PANEL_TEXT}`;
  if (/review|progress|running|active|open|wait|block|pending/i.test(word)) return `${PANEL_BLUE}◐ ${word}${PANEL_TEXT}`;
  return `${PANEL_MUTED}○ ${word}${PANEL_TEXT}`;
}

function entryItems(entries: readonly HistoryEntry[], query: string): PopupRow<Choice>[] {
  return entries.map(entry => ({
    value: { kind: "entry", entry },
    // The search ran on the board: its words always match its own results, further words narrow them here.
    match: [query, entry.id, entry.title, entry.status ?? "", JSON.stringify(entry.raw)].join(" "),
    lines: width => [split(`${bold(plainLabel(entry.id))}  ${plainLabel(entry.title)}`, statusMark(entry.status), width)],
  }));
}
/** The runs of a task's `corral show` read, latest first; each row keeps the task record. */
function runItems(view: RecordValue): PopupRow<Choice>[] {
  const task = record(view.task), runs = runEntries(view);
  if (!runs.length) return [{
    value: { kind: "task", task }, match: "no recorded runs",
    lines: width => [pad(`${PANEL_MUTED}no recorded runs · ⏎ shows the task record${PANEL_TEXT}`, width)],
  }];
  return runs.map(run => ({
    value: { kind: "run", task, run },
    match: [run.id, run.outcome ?? "", run.ended ?? ""].join(" "),
    lines: width => [split(`${bold(plainLabel(run.id))}  ${statusMark(run.outcome)}`,
      `${PANEL_MUTED}${run.ended === undefined ? "not ended" : `ended ${plainLabel(run.ended)}`}${PANEL_TEXT}`, width)],
  }));
}

/** Only a read callback and display API enter this navigator: it cannot replace or edit a session. */
export class HistoryNavigator {
  private busy = false;
  private disposed = false;
  /** Closes the open popup; set while it is open. */
  private close: (() => void) | undefined;
  constructor(private readonly read: HistoryRead | undefined, private readonly canInteract: () => boolean,
    private readonly notice: (text: string, tone: "text" | "warning" | "error") => void,
    private readonly place: () => OverlayOptions) {}
  dispose(): void { this.disposed = true; this.close?.(); }
  /**
   * Whether History may open its next surface: not disposed and the host free for it (no prompt, no other overlay,
   * the editor focused). Asked only while none of History's own overlays is mounted: on admission and between
   * surfaces, after the previous one closed (Pi hides it and restores focus before its promise resolves). A
   * foreign dialog opened during a read stops the flow there.
   */
  private get live(): boolean { return !this.disposed && this.canInteract(); }

  async open(ui: ExtensionUIContext): Promise<void> {
    if (this.busy || !this.live) return;
    const read = this.read;
    if (!read) { this.notice("History read API unavailable", "warning"); return; }
    this.busy = true;
    try {
      const choice = await this.choose(ui, read);
      if (!choice || !this.live) return;
      if (choice.kind === "run") await this.run(ui, read, choice.task, choice.run);
      else if (choice.kind === "task") await openPager(ui, this.taskOnly(choice.task));
    } catch (error) {
      if (!this.disposed) this.notice(`History unavailable: ${reason(error)}`, "warning");
    } finally { this.busy = false; this.close = undefined; }
  }

  /** The list popup: Enter runs a task search or opens a task's runs, ← or Esc leaves the runs. */
  private choose(ui: ExtensionUIContext, read: HistoryRead): Promise<Choice | undefined> {
    return ui.custom<Choice | undefined>((tui, _theme, _keys, done) => {
      let closed = false, runs: Runs | undefined, listing: Listing | undefined, filter = "";
      const finish = (value: Choice | undefined): void => {
        if (closed) return;
        closed = true; this.close = undefined; done(value);
      };
      this.close = () => finish(undefined);
      const changed = (): void => { if (!closed) tui.requestRender(); };
      /**
       * After a read inside the popup. The popup is History's own overlay, so the host reads as busy while it is
       * up: only closing and disposal end it here (`dispose()` closes it); nothing new opens from inside it.
       */
      const current = (): boolean => !closed && !this.disposed;
      const load = async (query: string): Promise<void> => {
        const next: Listing = { query, phase: "reading", items: [], shown: 0, returned: 0, error: undefined };
        listing = next; changed();
        try {
          const value = await read({ kind: "tasks", query });
          if (listing !== next || !current()) return;
          const { rows, returned } = historyEntries(value);
          listing = { ...next, phase: "read", items: entryItems(rows, query), shown: rows.length, returned };
        } catch (error) {
          if (listing !== next || !current()) return;
          listing = { ...next, phase: "failed", error: reason(error) };
        }
        changed();
      };
      const openRuns = async (entry: HistoryEntry): Promise<void> => {
        if (!runs) filter = popup.query;
        const view: Runs = { entry, phase: "reading", items: [], error: undefined };
        runs = view; popup.reset(); changed();
        try {
          const task = record(await read({ kind: "task", task: entry.id }));
          if (runs !== view || !current()) return;
          runs = { ...view, phase: "read", items: runItems(task) };
        } catch (error) {
          if (runs !== view || !current()) return;
          runs = { ...view, phase: "failed", error: reason(error) };
        }
        changed();
      };
      const popup: PopupList<Choice> = new PopupList<Choice>({
        get title() {
          if (runs) return `‹ ${plainLabel(runs.entry.id)} · Runs`;
          if (listing?.phase !== "read") return "History · read-only";
          const count = listing.shown < listing.returned ? `${listing.shown} of ${listing.returned} shown` : `${listing.returned} returned`;
          return `History · read-only · ${count} · not exhaustive`;
        },
        get placeholder() {
          return runs ? "filter runs" : "search tasks · ⏎ searches the board";
        },
        get empty() {
          if (runs) return runs.phase === "reading" ? `Reading ${plainLabel(runs.entry.id)}…`
            : runs.phase === "failed" ? `! ${plainLabel(runs.entry.id)} could not be read: ${runs.error ?? ""} · ⏎ retries` : "no run matches";
          return !listing || listing.phase === "reading" ? "Reading recorded history…"
            : listing.phase === "failed" ? `! History could not be read: ${listing.error ?? ""} · ⏎ retries` : "no recorded history matches";
        },
        right: () => runs ? `${PANEL_MUTED}← back${PANEL_TEXT}` : `${PANEL_MUTED}Tasks${PANEL_TEXT}`,
        rows: () => runs ? runs.items : listing?.items ?? [],
        capacity: () => Math.max(1, tui.terminal.rows - 8),
        key: (data, selected) => {
          if (runs) {
            if (matchesKey(data, "escape") || matchesKey(data, "left")) { runs = undefined; popup.reset(filter); return true; }
            if (matchesKey(data, "tab")) return true;
            if (matchesKey(data, "enter") && runs.phase !== "read") { if (runs.phase === "failed") void openRuns(runs.entry); return true; }
            return false;
          }
          if (matchesKey(data, "tab")) return true;
          if (!matchesKey(data, "enter")) return false;
          if (!listing || listing.phase === "reading") return true;
          const query = popup.query.trim();
          if (listing.phase === "failed" || query !== listing.query) { void load(query); return true; }
          if (selected?.kind === "entry") { void openRuns(selected.entry); return true; }
          return false;
        },
        done: finish,
        changed,
      });
      void load("");
      return popup;
    }, { overlay: true, overlayOptions: () => this.place() });
  }

  private taskOnly(task: RecordValue): PagerInput {
    return { title: `${plainLabel(String(task.id ?? ""))} · no recorded runs`, lines: ["No run has been recorded for this task.", "", NOT_INSTRUCTION],
      raw: rawLines(task), wrap: true };
  }

  /** The run's recap; `t` opens its transcript pages and returns to it, Esc closes. */
  private async run(ui: ExtensionUIContext, read: HistoryRead, task: RecordValue, run: RunEntry): Promise<void> {
    const id = String(task.id ?? "");
    const facts = taskRecap(run.raw);
    const recap = {
      title: `${plainLabel(id)} / ${plainLabel(run.id)} · recorded history`,
      lines: [...recapLines(facts), "", NOT_INSTRUCTION], raw: sourceLines(facts, run.raw),
      wrap: true, actions: RECAP_ACTIONS,
    };
    while (this.live) {
      const action = await openPager(ui, recap);
      if (!this.live || action !== "t") return;
      try { await this.transcript(ui, read, id, run.id); }
      catch (error) { if (!this.disposed) this.notice(`History unavailable: ${reason(error)}`, "warning"); }
    }
  }

  /** Bounded transcript pages: `n` continues after the page (at the byte cut when there is one), `p` goes back. */
  private async transcript(ui: ExtensionUIContext, read: HistoryRead, task: string, run: string): Promise<void> {
    const previous: number[] = [];
    let first = 1;
    while (this.live) {
      const text = String(await read({ kind: "transcript", task, run, first }));
      if (!this.live) return;
      const page = transcriptPage(text);
      const last = page.cut ? Math.max(first, nextHistoryPage(text, first) - 1) : first + TRANSCRIPT_PAGE - 1;
      const action = await openPager(ui, {
        title: `${plainLabel(task)} / ${plainLabel(run)} · transcript · records ${first}–${last}`,
        lines: page.empty ? ["End of the recorded transcript"] : page.lines,
        ...(page.empty ? {} : { raw: page.raw }),
        wrap: true,
        actions: [...(page.empty ? [] : [{ key: "n", label: "next page" }]), ...(previous.length ? [{ key: "p", label: "previous page" }] : [])],
      });
      if (action === "n" && !page.empty) { previous.push(first); first = nextHistoryPage(text, first); }
      else if (action === "p" && previous.length) first = previous.pop() ?? 1;
      else return;
    }
  }
}
