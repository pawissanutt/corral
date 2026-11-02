import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { Text, matchesKey } from "@earendil-works/pi-tui";
import type { HistoryRequest } from "../tools/history.ts";
import { pad, plainLabel } from "./panels-view.ts";
export type HistoryRead = (request: HistoryRequest) => Promise<unknown>;
type RecordValue = Record<string, unknown>;
export const record = (value: unknown): RecordValue => value !== null && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : {};
export const records = (value: unknown): RecordValue[] => Array.isArray(value) ? value.map(record) : [];
const field = (value: unknown): string => value === undefined || value === null || value === "" ? "Unknown / not recorded" : typeof value === "string" ? value : JSON.stringify(value);
const safe = (text: string): string => text.replace(/[\u0000-\u001f\u007f-\u009f]/g, char => char === "\n" ? char : " ");
export function nextHistoryPage(text: string, first: number): number {
  const marker = /\[page byte limit; resume with --lines (\d+)-\d+\]/.exec(text);
  return marker ? Number(marker[1]) : first + 200;
}
export function historyMetadata(value: unknown): string {
  const state = record(value);
  return JSON.stringify({ source: "recorded agent_host / assistant.container.state (not live health)",
    binding: state.binding ?? "Unknown / not recorded", generations: state.generations ?? "Unknown / not recorded",
    rewinds: state.rewinds ?? "Unknown / not recorded" }, null, 2);
}
function ledgerStep(value: unknown): unknown {
  const steps = records(record(value).phases).flatMap(phase => records(phase.steps));
  const step = steps.find(step => record(step.status).state === "in_progress") ?? steps.find(step => record(step.status).state === "pending");
  return step ? `${field(step.id)}: ${field(step.label)}` : undefined;
}
export function assistantRecap(session: RecordValue, container: RecordValue): string[] {
  const state = record(container.state);
  const questions = session.ended ? "None effective: assistant session ended" : Array.isArray(state.questions)
    ? records(state.questions).filter(question => question.state === "open" || question.state === "escalated") : undefined;
  return ["Last-known outcome: Unknown / no structured assistant outcome field",
    `Last-known summary: ${field(session.summary)} [assistant.session.summary]`,
    `Ended: ${field(session.ended)} [assistant.session.ended; missing is not success]`,
    `Pending question: ${field(questions)} [assistant.container.state.questions; ended-session closure]`,
    `Next recorded step: ${field(ledgerStep(state.todo))} [assistant.container.state.todo; first in-progress or pending ledger step, not an instruction]`];
}
export function historyRecap(task: RecordValue, run: RecordValue, questions: unknown): string[] {
  const checkpoint = record(record(records(run.checkpoints).at(-1)).checkpoint);
  const pending = Array.isArray(questions) ? records(questions).filter(view =>
    String(record(view.question).key ?? "").startsWith(`${String(task.id)}/${String(run.id)}/`)) : questions;
  return [
    `Last-known outcome: ${field(run.outcome ?? task.result)} [recorded task.result / run.outcome]`,
    `Summary: ${field(run.summary)} [run.summary]`,
    `Pending question: ${Array.isArray(pending) && !pending.length ? "None in effective pending readback" : field(pending)} [question list effective pending readback; selected run and children]`,
    `Next recorded step: ${field(record(checkpoint.input).next_action ?? ledgerStep(run.progress))} [run.checkpoints[-1].checkpoint.input.next_action, otherwise first in-progress/pending run.progress step; historical, not an instruction]`,
    `Checkpoint captured: ${field(checkpoint.captured)} [recorded checkpoint; compatibility not inferred]`,
  ];
}
/** Only a read callback and display API enter this navigator: it cannot replace or edit a session. */
export class HistoryNavigator {
  private busy = false;
  private disposed = false;
  constructor(private readonly read: HistoryRead | undefined, private readonly canInteract: () => boolean) {}
  dispose(): void { this.disposed = true; }
  async open(ui: ExtensionUIContext): Promise<void> {
    if (this.busy || this.disposed || !this.canInteract()) return;
    if (!this.read) { ui.notify("History read API unavailable", "warning"); return; }
    this.busy = true;
    try {
      const kind = await ui.select("Read-only history · current session and draft stay bound", ["Tasks / runs", "Assistant sessions"]);
      if (!kind || this.disposed) return;
      const query = await ui.input("Search recorded history", "Task title / ID or assistant ID / focus");
      if (query === undefined || this.disposed) return;
      const assistants = kind === "Assistant sessions";
      const source = records(await this.read(assistants ? { kind: "assistants" } : { kind: "tasks", query }));
      if (this.disposed) return;
      const rows = assistants ? source.filter(row => JSON.stringify(row).toLowerCase().includes(query.toLowerCase())) : source;
      // The backend process has a hard byte bound. Never claim this visible slice is exhaustive.
      const choices = rows.slice(0, 200).map(row => `${field(row.id ?? row.session)} · ${plainLabel(field(row.title ?? row.focus))} · ${field(row.status ?? row.ended ?? "open (recorded)")}`);
      if (!choices.length) { ui.notify("No matching recorded history", "info"); return; }
      const choice = await ui.select(`Select history (${choices.length}/${rows.length} returned records)`, choices);
      const selected = rows[choices.indexOf(choice ?? "")];
      if (!selected || this.disposed) return;
      if (assistants) {
        const value = record(await this.read({ kind: "assistant", assistant: String(selected.session) }));
        const session = record(value.session), container = record(value.container);
        await this.show(ui, `Assistant ${field(session.id)} · read-only`, [
          ...assistantRecap(session, container),
          "Transcript: unavailable here; existing pageable transcript API supports task runs only.",
          "Recorded generations and recovery (no restore controls):", historyMetadata(container.state),
        ].join("\n"));
        return;
      }
      const task = record(await this.read({ kind: "task", task: String(selected.id) }));
      if (this.disposed) return;
      const runs = records(task.runs).reverse();
      const labels = runs.map(run => `${field(run.id)} · ${field(run.outcome)} · ${field(run.ended ?? "not ended in record")}`);
      const label = labels.length ? await ui.select(`Runs of ${field(task.id)} (recorded)`, labels) : undefined;
      if (this.disposed) return;
      const run = runs[labels.indexOf(label ?? "")];
      if (!run) {
        if (!runs.length) await this.show(ui, `${field(task.id)} · no recorded runs`, `Last-known result: ${field(task.result)} [task.result]\nPending question / next step: Unknown / no run selected`);
        return;
      }
      let questions: unknown;
      try { questions = await this.read({ kind: "questions", task: String(task.id) }); }
      catch (error) { questions = `Unavailable: ${String(error)}`; }
      const recap = historyRecap(task, run, questions);
      const action = await this.show(ui, `${field(task.id)}/${field(run.id)} · recorded history`, [
        ...recap, "", "Recorded generations / recovery (unknown when absent):", historyMetadata(run.agent_host),
        "", "t: transcript pages · c: available native checkpoints/rewinds · Esc: close",
      ].join("\n"), true);
      if (this.disposed) return;
      if (action === "c") {
        const recovery = await this.read({ kind: "recovery", task: String(task.id), run: String(run.id) });
        await this.show(ui, "Native checkpoint / rewind history · read-only", JSON.stringify(recovery, null, 2));
      } else if (action === "t") await this.transcript(ui, String(task.id), String(run.id));
    } catch (error) {
      if (!this.disposed) ui.notify(`History unavailable: ${plainLabel(String(error))}`, "warning");
    } finally { this.busy = false; }
  }
  private async transcript(ui: ExtensionUIContext, task: string, run: string): Promise<void> {
    const previous: number[] = []; let first = 1;
    while (!this.disposed && this.read) {
      const text = String(await this.read({ kind: "transcript", task, run, first }));
      const action = await this.show(ui, `${task}/${run} · records ${first}-${first + 199} · bounded raw JSONL`,
        text || "End of available transcript records", true, true);
      if (action === "n" && text) { previous.push(first); first = nextHistoryPage(text, first); }
      else if (action === "p" && previous.length) first = previous.pop() ?? 1;
      else if (action !== "p" && action !== "n") return;
    }
  }
  private async show(ui: ExtensionUIContext, title: string, text: string, actions = false, paging = false): Promise<string | undefined> {
    if (this.disposed) return undefined;
    const truncated = text.length > 64 * 1024;
    return ui.custom<string | undefined>((tui, _theme, _keys, done) => {
      const content = new Text(safe(text.slice(0, 64 * 1024)) + (truncated ? "\n[display truncated; recorded data is larger]" : ""), 0, 0);
      let offset = 0, max = 0;
      return {
        render: width => {
          const lines = content.render(width), height = Math.max(1, Math.floor(tui.terminal.rows * 0.8) - 2);
          max = Math.max(0, lines.length - height); offset = Math.min(offset, max);
          return [pad(plainLabel(title), width), ...lines.slice(offset, offset + height), pad(paging ? "p previous page · n next page · ↑↓ scroll · Esc close" : actions ? "t transcript · c checkpoints · ↑↓ scroll · Esc close" : "↑↓ scroll · Esc close", width)];
        },
        handleInput: data => {
          if (matchesKey(data, "escape") || data === "q") { done(undefined); return; }
          if (actions && (paging ? data === "n" || data === "p" : data === "t" || data === "c")) { done(data); return; }
          if (matchesKey(data, "up")) offset = Math.max(0, offset - 1);
          if (matchesKey(data, "down")) offset = Math.min(max, offset + 1);
          if (matchesKey(data, "pageUp")) offset = Math.max(0, offset - 10);
          if (matchesKey(data, "pageDown")) offset = Math.min(max, offset + 10);
          tui.requestRender();
        }, invalidate() { content.invalidate(); },
      };
    }, { overlay: true, overlayOptions: { width: "90%", maxHeight: "85%", anchor: "center" } });
  }
}
