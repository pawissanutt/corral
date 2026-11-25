import { matchesKey, truncateToWidth, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { actionRows, type Action, type AgentsAction, type AgentsHit } from "./agents-view.ts";
import type { JobRow } from "./agents-model.ts";
import { FittedScrollView, jobDetailLines, type JobFreeze } from "./job-detail.ts";
import { FILTER_KEY, keyLabel } from "./keymap.ts";
import { PANEL_BG, PANEL_BLUE, PANEL_WARNING, RESET } from "./panels-view.ts";

interface JobPageHost {
  entry(): JobRow | undefined; bound(): boolean; now(): number; legend(): boolean;
  freeze(): JobFreeze | undefined; click(event: TuiMouseEvent): boolean;
  back(): void; focus(): void; act(action: AgentsAction, key: string): void;
}
const BACK = "‹ Agents";

/** A command's reading area fits the newest retained lines to its viewport. */
export class JobPage {
  readonly scroll: FittedScrollView;
  private hits: AgentsHit[][] = [];
  private actionTop = 0;
  constructor(private readonly host: JobPageHost) {
    this.scroll = new FittedScrollView({ render: width => this.render(width), invalidate() {}, handleMouse: event => this.mouse(event) },
      { primary: true, follow: "none", scrollbar: "auto" });
  }
  private caps(entry: JobRow | undefined): Action[] {
    const frozen = !!entry && this.host.freeze()?.job === entry.job;
    return ([
      ["search", keyLabel(FILTER_KEY), "search output", entry?.capabilities.search],
      ["follow", "f", frozen ? "resume follow" : "pause follow", entry?.capabilities.follow || frozen],
      ["peek", "p", "peek", entry?.capabilities.peek],
      ["background", "b", "background", entry?.capabilities.background],
      ["stop", "x", "stop", entry?.capabilities.stop],
      ["legend", "?", this.host.legend() ? "hide legend" : "legend", !!entry],
    ] as const).flatMap(([action, key, label, enabled]) => enabled ? [{ action, key, label, enabled }] : []);
  }
  actions(): AgentsAction[] { return this.caps(this.host.entry()).map(item => item.action); }
  handleKey(data: string): boolean {
    const entry = this.host.entry();
    const action = this.caps(entry).find(item => item.action === "search" ? matchesKey(data, FILTER_KEY) : data === item.key);
    if (!entry || !action) return false;
    this.host.act(action.action, entry.key); return true;
  }
  private render(width: number): string[] {
    const entry = this.host.entry(), room = Math.max(1, width - 2);
    const frame = (line: string) => {
      const text = truncateToWidth(" " + line, width, "…");
      return PANEL_BG + text + " ".repeat(Math.max(0, width - visibleWidth(text))) + RESET;
    };
    const actions = actionRows(entry?.key ?? "", this.caps(entry), width);
    const detail = entry ? jobDetailLines(entry.source, { bound: this.host.bound(), now: this.host.now(), legend: this.host.legend(),
      freeze: this.host.freeze(), page: true }, room, this.scroll.viewportHeight - 1 - actions.lines.length)
      : [PANEL_WARNING + "The selected command is no longer retained"];
    this.hits = actions.hits; this.actionTop = detail.length + 1;
    return [frame(PANEL_BLUE + BACK), ...detail.map(frame), ...actions.lines];
  }
  private mouse(event: TuiMouseEvent): { readonly handled: boolean } | undefined {
    if (!this.host.click(event)) return undefined;
    this.host.focus();
    if (event.y === 0 && event.x >= 1 && event.x < 1 + visibleWidth(BACK)) this.host.back();
    else {
      const hit = this.hits[event.y - this.actionTop]?.find(item => event.x >= (item.from ?? Infinity) && event.x < (item.to ?? 0));
      if (hit?.action) this.host.act(hit.action, hit.key);
    }
    return { handled: true };
  }
}
