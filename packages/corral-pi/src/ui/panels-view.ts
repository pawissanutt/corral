// Native presentation only. All values come from the bound host or an explicit Git read.
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { ContextUsage, Theme } from "@earendil-works/pi-coding-agent";
import type { HostIdentity } from "../roles/types.ts";
import type { TodoProgress } from "./todo-widget.ts";
import { inspectorActivity, inspectorContext, type InspectorSnapshot } from "./inspector.ts";

export const PANEL_TABS = ["Context", "Files", "Agents", "Plan"] as const;
export type PanelTab = typeof PANEL_TABS[number];
export interface PanelJob { readonly job: string; readonly state: string; readonly detail?: import("../extensions/jobs/runtime.ts").JobInspection }
export interface PanelSnapshot {
  readonly inspector?: InspectorSnapshot;
  readonly bound: boolean;
  readonly model: string | null;
  readonly effort: string;
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
  readonly action?: { readonly kind: "diff"; readonly path: string } | { readonly kind: "files" };
}
export const PANEL_BG = "\x1b[48;2;25;27;32m";
export const PANEL_TEXT = "\x1b[38;2;224;227;233m";
export const PANEL_MUTED = "\x1b[38;2;155;162;176m";
export const PANEL_BLUE = "\x1b[38;2;131;168;239m";
export const PANEL_RULE = "\x1b[38;2;65;71;82m";
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
      if (!snapshot.bound) add("Disconnected/unbound · observations may be stale");
      if (!snapshot.inspector) add("Host inspector unavailable · legacy snapshot");
      add(`${snapshot.tools.length} active tools · ${snapshot.skills.length} skills`);
      add(`Active tools: ${snapshot.tools.join(", ") || "none"}`);
      for (const path of snapshot.contextFiles) add(path);
      if (!snapshot.contextFiles.length) add("No context files loaded");
      if (snapshot.skills.length) add(`Skills: ${snapshot.skills.join(", ")}`);
      add(`${snapshot.tokens.toLocaleString()} recorded session tokens`);
      if (snapshot.inspector) for (const line of inspectorContext(snapshot.inspector)) add(line);
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
    case "Agents": {
      add("Current bound session", true);
      add(`${snapshot.model ?? "Model unavailable"} · ${snapshot.effort}`);
      add(`${identity.role} · ${identity.task ?? identity.assistant ?? identity.project}`);
      add(`${identity.review ?? identity.run ?? ""} · g${identity.generation} · ${snapshot.streaming ? "Working" : "Idle"}`);
      add(snapshot.bound ? "Bound" : "Disconnected/unbound · observations may be stale");
      if (snapshot.inspector) {
        add(`Session ${snapshot.inspector.session} · bound ${snapshot.inspector.boundSession ?? "unavailable"}`);
        add(`Turn ${snapshot.inspector.turn} · ${snapshot.inspector.turnOpen ? "open" : "settled"} · ${snapshot.streaming ? "streaming" : "not streaming"}`);
        for (const line of inspectorActivity(snapshot.inspector)) add(line);
      }
      else add("Receipt/event history unavailable · legacy snapshot");
      add("Read-only auxiliary activity", true);
      if (!snapshot.auxiliary) add("Auxiliary telemetry unavailable");
      else {
        add(`Research: ${snapshot.auxiliary.explore.running} slots occupied · ${snapshot.auxiliary.explore.queued} queued`);
        add(`Oracle: ${snapshot.auxiliary.oracle.running} slots occupied · ${snapshot.auxiliary.oracle.queued} queued`);
      }
      add("Inspection does not resume agents");
      break;
    }
    case "Plan": {
      const ledger = snapshot.ledger;
      add("Canonical plan", true);
      if (snapshot.ledgerStale && ledger) add("Canonical plan stale · refresh to reconcile unconfirmed state");
      if (!snapshot.bound) add("Disconnected/unbound · retained ledger may be stale");
      if (!ledger) { add(snapshot.ledgerStale ? "Canonical plan unavailable" : "No canonical plan recorded"); break; }
      add(`${ledger.plan ?? "Progress"} · revision ${ledger.revision}`);
      for (const phase of ledger.phases) {
        add(phase.label, true);
        for (const step of phase.steps) {
          const state = step.status;
          const mark = { pending: "○", in_progress: "●", done: "✓", dropped: "–" }[state.state];
          add(`${mark} ${step.label}`);
          if (state.state === "dropped") add(`Reason: ${state.reason}`);
          for (const criterion of step.criteria) add(`  Criterion: ${criterion}`);
          for (const wait of ledger.waits?.filter(wait => wait.step === step.id) ?? []) {
            add(wait.on.kind === "owner" ? `Waits on owner: ${wait.on.question}` : `Waits on event: ${wait.on.event}`);
          }
        }
      }
      add("Self-reported progress, not gate evidence");
      break;
    }
  }
  return rows;
}
