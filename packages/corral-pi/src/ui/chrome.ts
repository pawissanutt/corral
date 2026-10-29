// Pane chrome for the rich UI: a two-line identity header, the terminal title and two footer
// statuses (corral binding, and refusals that need the owner). It reads the launcher-bound
// identity and the host event log only; it never calls corral or changes the session.
import { basename } from "node:path";

import type { ExtensionAPI, ExtensionUIContext, Theme } from "@earendil-works/pi-coding-agent";
import { truncateToWidth } from "@earendil-works/pi-tui";

import type { BridgeEvent, EventLog } from "../bridge/events.ts";
import type { HostIdentity, Role } from "../roles/types.ts";
import { type ThemeClass, draculaTheme } from "./dracula.ts";

type Style = Pick<Theme, "fg" | "bold">;

export interface ChromeInfo {
  readonly identity: HostIdentity;
  readonly provider: string;
  readonly model: string;
  readonly effort?: string;
  readonly cwd: string;
}

export interface Alerts {
  networkRefused: number;
  toolsDenied: number;
}

export const STATUS_BINDING = "corral-1-binding";
export const STATUS_ALERTS = "corral-2-alerts";

const ROLE_LABEL: Record<Role, string> = {
  worker: "WORKER", reviewer: "REVIEWER", pm: "PM", planner: "PLANNER", plan_reviewer: "PLAN REVIEWER",
};
const ROLE_COLOR: Record<Role, "accent" | "warning" | "success"> = {
  worker: "accent", reviewer: "warning", pm: "success", planner: "accent", plan_reviewer: "warning",
};

export function target(identity: HostIdentity): string {
  if (!identity.task) return identity.project;
  const run = identity.review ?? identity.run;
  return run ? `${identity.task} · ${run}` : identity.task;
}

export function modelLabel(info: Pick<ChromeInfo, "provider" | "model">): string {
  return info.model.startsWith(`${info.provider}/`) ? info.model : `${info.provider}/${info.model}`;
}

export function headerLines(info: ChromeInfo, theme: Style): string[] {
  const { identity } = info;
  const badge = theme.fg(ROLE_COLOR[identity.role], theme.bold(`▌${ROLE_LABEL[identity.role]}`));
  const detail = [modelLabel(info), info.effort, basename(info.cwd)].filter(Boolean).join(" · ");
  return [
    `${badge}  ${theme.bold(target(identity))}  ${theme.fg("dim", `g${identity.generation}`)}`,
    theme.fg("dim", detail),
  ];
}

export function titleText(identity: HostIdentity): string {
  return identity.task ? `${identity.task} ${identity.role}` : `corral ${identity.role}`;
}

export function bindingStatus(bound: boolean, generation: number, theme: Style): string {
  return bound ? theme.fg("success", `● corral g${generation}`) : theme.fg("warning", "○ corral unbound");
}

export function alertsStatus(alerts: Alerts, theme: Style): string | undefined {
  const parts: string[] = [];
  if (alerts.networkRefused > 0) parts.push(`${alerts.networkRefused} network refused`);
  if (alerts.toolsDenied > 0) parts.push(`${alerts.toolsDenied} tool denied`);
  return parts.length > 0 ? theme.fg("warning", `⚠ ${parts.join(" · ")}`) : undefined;
}

export function countAlert(alerts: Alerts, event: Pick<BridgeEvent, "kind">): boolean {
  if (event.kind === "network_refused") alerts.networkRefused++;
  else if (event.kind === "tool_denied") alerts.toolsDenied++;
  else return false;
  return true;
}

export function installChrome(
  pi: ExtensionAPI, info: ChromeInfo, events: EventLog, bound: () => boolean, themeClass: ThemeClass,
): void {
  const alerts: Alerts = { networkRefused: 0, toolsDenied: 0 };
  let ui: ExtensionUIContext | undefined;
  const refresh = () => {
    if (!ui) return;
    ui.setStatus(STATUS_BINDING, bindingStatus(bound(), info.identity.generation, ui.theme));
    ui.setStatus(STATUS_ALERTS, alertsStatus(alerts, ui.theme));
  };
  events.on((event) => {
    if (countAlert(alerts, event) || event.kind === "bound") refresh();
  });
  pi.on("session_start", (_event, ctx) => {
    if (ctx.mode !== "tui") return;
    ui = ctx.ui;
    ui.setTheme(draculaTheme(themeClass, ui.theme.getColorMode()));
    ui.setTitle(titleText(info.identity));
    ui.setHeader((_tui, theme) => ({
      render: (width: number) => headerLines(info, theme).map((line) => truncateToWidth(line, width)),
      invalidate: () => {},
    }));
    refresh();
  });
}
