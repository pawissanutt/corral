// Native presentation only. All values come from the bound host or an explicit Git read.
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { ContextUsage, Theme } from "@earendil-works/pi-coding-agent";
import type { HostIdentity } from "../roles/types.ts";
import type { TodoProgress } from "./todo-widget.ts";
import { inspectorActivity, inspectorContext, type InspectorSnapshot } from "./inspector.ts";
import { ansi, PACKAGED, type Appearance } from "./appearance.ts";

export const PANEL_TABS = ["Context", "Files", "Plan"] as const;
export type PanelTab = typeof PANEL_TABS[number];
export interface PanelJob { readonly job: string; readonly state: string; readonly detail?: import("../extensions/jobs/runtime.ts").JobInspection }
export interface PanelSnapshot {
  readonly inspector?: InspectorSnapshot;
  readonly budget?: unknown;
  readonly agents?: readonly import("./agent-view.ts").AgentObservation[];
  readonly bound: boolean;
  readonly model: string | null;
  readonly effort: string;
  readonly effortAvailable?: boolean;
  readonly input?: import("../host/input-observation.ts").InputObservation;
  readonly streaming: boolean;
  readonly context: ContextUsage | undefined;
  readonly tokens: number;
  readonly tools: readonly string[];
  readonly contextFiles: readonly string[];
  readonly skills: readonly string[];
  readonly ledger: TodoProgress | null;
  readonly ledgerStale: boolean;
  readonly jobs: readonly PanelJob[] | null;
  readonly auxiliary: { explore: { running: number; queued: number }; oracle: { running: number; queued: number } } | null;
  readonly helpers?: readonly import("../extensions/subagent/inspection.ts").HelperInspection[] | null;
  readonly steering: number;
  readonly followUps: number;
}
export interface PanelFile { readonly path: string; readonly status: string; readonly originalPath?: string }
export type FilesState = { readonly kind: "unread" | "loading" } |
  { readonly kind: "ready"; readonly files: readonly PanelFile[] } |
  { readonly kind: "error"; readonly reason: string };
export interface PanelLine {
  readonly text: string;
  readonly heading?: boolean;
  readonly tone?: "text" | "muted" | "accent" | "success" | "warning";
  readonly indent?: number;
  readonly continuationIndent?: number;
  readonly action?: { readonly kind: "diff"; readonly path: string } | { readonly kind: "files" } | { readonly kind: "scope" } | { readonly kind: "helpers" } | { readonly kind: "helper"; readonly request: string } | { readonly kind: "job"; readonly job: string } | { readonly kind: "history" };
}
export let PANEL_BG = ansi(PACKAGED.dark.background, true);
export let PANEL_TEXT = ansi(PACKAGED.dark.text);
export let PANEL_MUTED = ansi(PACKAGED.dark.muted);
export let PANEL_BLUE = ansi(PACKAGED.dark.accent);
export let PANEL_RULE = ansi(PACKAGED.dark.rule);
export let PANEL_SUCCESS = ansi(PACKAGED.dark.success);
export let PANEL_WARNING = ansi(PACKAGED.dark.warning);
export let PANEL_ERROR = ansi(PACKAGED.dark.error);
export let PANEL_SELECTED = ansi(PACKAGED.dark.selected, true);
/** One native host per process; live bindings keep every panel on the same packaged palette. */
export function setPanelAppearance(appearance: Appearance): void {
  const p = PACKAGED[appearance];
  PANEL_BG = ansi(p.background, true); PANEL_TEXT = ansi(p.text); PANEL_MUTED = ansi(p.muted);
  PANEL_BLUE = ansi(p.accent); PANEL_RULE = ansi(p.rule); PANEL_SUCCESS = ansi(p.success); PANEL_WARNING = ansi(p.warning); PANEL_ERROR = ansi(p.error); PANEL_SELECTED = ansi(p.selected, true);
}
export function styledPanelLine(row: PanelLine): string {
  const color = row.tone === "muted" ? PANEL_MUTED : row.tone === "success" ? PANEL_SUCCESS
    : row.tone === "warning" ? PANEL_WARNING : row.tone === "accent" ? PANEL_BLUE : PANEL_TEXT;
  return color + (row.heading ? "\x1b[1m" : "") + " ".repeat(row.indent ?? 0) + row.text + "\x1b[22m" + PANEL_TEXT;
}
export const RESET = "\x1b[0m";

export function plainLabel(text: string): string {
  return text.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").replace(/\s+/gu, " ").trim();
}
export function pad(text: string, width: number): string {
  const clipped = truncateToWidth(text, Math.max(0, width), "…");
  return clipped + " ".repeat(Math.max(0, width - visibleWidth(clipped)));
}
export function box(title: string, lines: readonly string[], width: number, height: number, focus = false): string[] {
  if (width < 4 || height < 2) return [];
  const rule = focus ? PANEL_BLUE : PANEL_RULE;
  const heading = pad(` ${plainLabel(title)} `, width - 4).trimEnd();
  const result = [`${PANEL_BG}${rule}┌─${heading}${"─".repeat(Math.max(0, width - 3 - visibleWidth(heading)))}┐${RESET}`];
  for (let row = 0; row < height - 2; row++) {
    result.push(`${PANEL_BG}${rule}│${PANEL_TEXT} ${pad(lines[row] ?? "", width - 4)}${PANEL_BG} ${rule}│${RESET}`);
  }
  result.push(`${PANEL_BG}${rule}└${"─".repeat(width - 2)}┘${RESET}`);
  return result;
}
export function diffLine(text: string, theme: Pick<Theme, "fg">, inHunk = false): string {
  const safe = text.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ");
  if (!inHunk && /^(diff |index |---|\+\+\+)/.test(safe)) return theme.fg("muted", safe);
  if (safe.startsWith("@@")) return theme.fg("accent", safe);
  if (safe.startsWith("+")) return theme.fg("toolDiffAdded", safe);
  if (safe.startsWith("-")) return theme.fg("toolDiffRemoved", safe);
  return safe;
}
export function panelLines(tab: PanelTab, identity: HostIdentity, snapshot: PanelSnapshot | null, files: FilesState): PanelLine[] {
  if (!snapshot) return [{ text: "Session state unavailable" }];
  const rows: PanelLine[] = [];
  const add = (text: string, heading = false) => rows.push({ text: plainLabel(text), heading });
  switch (tab) {
    case "Context": {
      add("Loaded context", true);
      add(`${identity.role.replaceAll("_", " ")} · generation ${identity.generation}`);
      add(`${snapshot.streaming ? "Working" : "Idle"} · ${snapshot.bound ? "Bound to Corral" : "Disconnected/unbound"}`);
      if (!snapshot.bound) add("Disconnected/unbound · observations may be stale");
      if (!snapshot.inspector) add("Host inspector unavailable · legacy snapshot");
      add(`${snapshot.tools.length} active tools · ${snapshot.skills.length} skills`);
      add(`Active tools: ${snapshot.tools.join(", ") || "none"}`);
      for (const path of snapshot.contextFiles) add(path);
      if (!snapshot.contextFiles.length) add("No context files loaded");
      if (snapshot.skills.length) add(`Skills: ${snapshot.skills.join(", ")}`);
      add(`${snapshot.tokens.toLocaleString()} recorded session tokens`);
      if (snapshot.inspector) {
        for (const line of inspectorContext(snapshot.inspector)) add(line);
        add("Session activity", true);
        add(`Session ${snapshot.inspector.session} · bound ${snapshot.inspector.boundSession ?? "unavailable"}`);
        add(`Turn ${snapshot.inspector.turn} · ${snapshot.inspector.turnOpen ? "open" : "settled"}`);
        for (const line of inspectorActivity(snapshot.inspector)) add(line);
      }
      add("Jobs", true);
      if (snapshot.jobs === null) add("Background jobs unavailable");
      else if (!snapshot.jobs.length) add("No background jobs");
      else for (const job of snapshot.jobs) add(`${job.job} · ${job.state}`);
      break;
    }
    case "Files":
      add("Git working changes", true);
      switch (files.kind) {
        case "unread": add("Enter Files or /panels refresh"); break;
        case "loading": add("Reading Git status…"); break;
        case "error": add(files.reason); break;
        case "ready":
          for (const file of files.files) add(`${file.status} ${file.path}`);
          if (!files.files.length) add("Working tree clean");
          if (files.files.length === 500) add("Showing the first 500 changes");
          add("Enter opens a read-only diff");
          break;
      }
      break;
    case "Plan": {
      const ledger = snapshot.ledger;
      add(`Session plan · ${identity.task ?? identity.assistant ?? identity.role}`, true);
      add("Files scope does not filter this plan");
      if (snapshot.ledgerStale && ledger) add("Canonical plan stale · refresh to reconcile unconfirmed state");
      if (!snapshot.bound) add("Disconnected/unbound · retained ledger may be stale");
      if (!ledger) { add(snapshot.ledgerStale ? "Canonical plan unavailable" : "No canonical plan recorded"); break; }
      const steps = ledger.phases.flatMap(phase => phase.steps);
      const done = steps.filter(step => step.status.state === "done").length;
      const dropped = steps.filter(step => step.status.state === "dropped").length;
      add(`${ledger.plan ?? "Progress"} · revision ${ledger.revision}`);
      add(`${done} done · ${steps.length - done - dropped} open${dropped ? ` · ${dropped} dropped` : ""}`);
      for (const phase of ledger.phases) {
        add("");
        add(phase.label, true);
        for (const step of phase.steps) {
          const state = step.status;
          const mark = { pending: "○", in_progress: "●", done: "✓", dropped: "–" }[state.state];
          const waits = ledger.waits?.filter(wait => wait.step === step.id) ?? [];
          rows.push({ text: plainLabel(`${mark} ${step.label}`), continuationIndent: 2, tone: waits.length ? "warning"
            : state.state === "done" || state.state === "dropped" ? "muted" : state.state === "in_progress" ? "accent" : "text" });
          if (state.state === "dropped") rows.push({ text: plainLabel(`Reason: ${state.reason}`), indent: 2, tone: "muted" });
          for (const criterion of step.criteria) rows.push({ text: plainLabel(`Criterion: ${criterion}`), indent: 2, tone: "muted" });
          for (const wait of waits) rows.push({ text: plainLabel(wait.on.kind === "owner" ? `Owner: ${wait.on.question}` : `Waiting: ${wait.on.event}`), indent: 2, tone: "warning" });
        }
      }
      add("Self-reported progress, not gate evidence");
      break;
    }
  }
  return rows;
}
