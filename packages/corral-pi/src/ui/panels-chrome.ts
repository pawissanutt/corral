// One owner per ambient datum: model in the composer, context in rail OR footer.
import { basename } from "node:path";
import { stripTerminalSequences, visibleWidth, Text } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { STATUS_BINDING, STATUS_ALERTS, target } from "./chrome.ts";
import {
  pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_TEXT, RESET,
  type PanelSnapshot, type FilesState, type PanelLine,
} from "./panels-view.ts";

export function splitRow(left: string, right: string, width: number): string {
  const tail = pad(right, Math.min(width, visibleWidth(right)));
  const room = Math.max(0, width - visibleWidth(tail) - (tail ? 2 : 0));
  return pad(pad(left, room) + (tail ? "  " + tail : ""), width);
}
export function contextLabel(snapshot: PanelSnapshot | null): string {
  const usage = snapshot?.context;
  if (usage?.tokens == null) return "Context unavailable";
  const compact = (n: number) => n >= 1000 ? `${Number((n / 1000).toFixed(1))}k` : String(n);
  return `${compact(usage.tokens)}/${compact(usage.contextWindow)} · ${usage.percent == null ? "context usage unknown" : Math.round(usage.percent) + "% context"}`;
}
export function panelHeader(identity: HostIdentity, cwd: string, branch: string | null,
  snapshot: PanelSnapshot | null, width: number): string[] {
  return [
    PANEL_BG + splitRow(PANEL_BLUE + "corral / " + plainLabel(target(identity)),
      PANEL_MUTED + (!snapshot ? "Unavailable" : snapshot.streaming ? "Working" : "Idle"), width) + RESET,
    PANEL_BG + PANEL_MUTED + pad(`${identity.role.replaceAll("_", " ").toUpperCase()} · g${identity.generation} · ` +
      (snapshot?.bound ? "Bound to Corral · " : "Unbound · ") +
      plainLabel(basename(cwd)) + (branch ? ` (${plainLabel(branch)})` : ""), width) + RESET,
  ];
}
export function composerBar(snapshot: PanelSnapshot | null, width: number, hidden = 0): string {
  const full = snapshot?.model;
  const model = full ? full.slice(full.indexOf("/") + 1) : "Model unavailable";
  const effort = snapshot ? ` · ${plainLabel(snapshot.effort)}` : "";
  const hint = hidden > 0 ? `↓ ${hidden} more` : "Enter send";
  const room = Math.max(0, width - 6 - hint.length - effort.length - 2);
  return PANEL_MUTED + "└ " + PANEL_TEXT + splitRow(
    pad(plainLabel(model), Math.min(room, visibleWidth(model))).trimEnd() + PANEL_MUTED + effort,
    PANEL_MUTED + hint, Math.max(0, width - 4)) + PANEL_MUTED + " ┘";
}
export function panelFooter(snapshot: PanelSnapshot | null, contextInRail: boolean,
  statuses: ReadonlyMap<string, string>, width: number): string[] {
  const queue = snapshot && snapshot.steering + snapshot.followUps > 0
    ? `${snapshot.steering} steering · ${snapshot.followUps} follow-ups` : "";
  const lines = contextInRail && !queue ? []
    : [PANEL_MUTED + splitRow(contextInRail ? "" : contextLabel(snapshot), queue, width) + RESET];
  // Binding already lives in the header. Never lose refusals or other extension statuses.
  const alerts = [...statuses].filter(([key]) => key !== STATUS_BINDING)
    .sort(([a], [b]) => Number(b === STATUS_ALERTS) - Number(a === STATUS_ALERTS));
  for (const [, value] of alerts) {
    const text = plainLabel(stripTerminalSequences(value));
    lines.push(...new Text(text, 0, 0).render(Math.max(1, width)).map(line => PANEL_TEXT + line + RESET));
  }
  return lines;
}
export function overviewLines(snapshot: PanelSnapshot | null, files: FilesState): PanelLine[] {
  if (!snapshot) return [{ text: "Session state unavailable" }];
  const rows: PanelLine[] = [{ text: "Context", heading: true }, { text: contextLabel(snapshot) },
    { text: "" }, { text: "Changed files", heading: true }];
  switch (files.kind) {
    case "unread": rows.push({ text: "Open Files to read Git changes" }); break;
    case "loading": rows.push({ text: "Reading Git status…" }); break;
    case "error": rows.push({ text: files.reason }); break;
    case "ready":
      for (const file of files.files.slice(0, 4)) rows.push({ text: plainLabel(`${file.status} ${file.path}`), action: { kind: "diff", path: file.path } });
      if (!files.files.length) rows.push({ text: "Working tree clean" });
      if (files.files.length > 4) rows.push({ text: `${files.files.length - 4} more in Files`, action: { kind: "files" } });
      break;
  }
  rows.push({ text: "" }, { text: "Agents & jobs", heading: true });
  if (snapshot.auxiliary) rows.push({ text: `Helper slots: research ${snapshot.auxiliary.explore.running} · oracle ${snapshot.auxiliary.oracle.running} occupied` });
  if (snapshot.jobs === null) rows.push({ text: "Background jobs unavailable" });
  else if (!snapshot.jobs.length) rows.push({ text: "No background jobs" });
  else for (const job of snapshot.jobs.slice(0, 3)) rows.push({ text: plainLabel(`${job.job} · ${job.state}`) });
  rows.push({ text: "" }, { text: "Plan", heading: true });
  const steps = snapshot.ledger?.phases.flatMap(phase => phase.steps);
  if (!steps?.length) rows.push({ text: snapshot.ledgerStale ? "Canonical plan unavailable" : "No plan recorded" });
  else {
    rows.push({ text: `${steps.filter(step => step.status.state === "done").length}/${steps.length} steps${snapshot.ledgerStale ? " · stale" : ""}` });
    for (const step of steps.filter(step => step.status.state === "in_progress" || step.status.state === "pending").slice(0, 3)) {
      rows.push({ text: plainLabel(`${step.status.state === "in_progress" ? "●" : "○"} ${step.label}`) });
    }
  }
  return rows;
}
