// Presentation only: the Rust ledger owns statuses; clicks only change disclosure and scroll.
import type { Theme } from "@earendil-works/pi-coding-agent";
import { Text, truncateToWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { z } from "zod";

const Status = z.discriminatedUnion("state", [
  z.object({ state: z.literal("pending") }),
  z.object({ state: z.literal("in_progress") }),
  z.object({ state: z.literal("done") }),
  z.object({ state: z.literal("dropped"), reason: z.string() }),
]);
export const ProgressSchema = z.object({
  revision: z.number().int().nonnegative(),
  spec_revision: z.string(),
  phases: z.array(z.object({
    id: z.string(), label: z.string(),
    steps: z.array(z.object({
      id: z.string(), label: z.string(), status: Status,
      criteria: z.array(z.string()).default([]),
    })),
  })),
});
export type TodoProgress = z.infer<typeof ProgressSchema>;
type Step = TodoProgress["phases"][number]["steps"][number];
type Style = Pick<Theme, "fg" | "bold">;

function mark(status: Step["status"], theme: Style): string {
  switch (status.state) {
    case "done": return theme.fg("success", "✓");
    case "in_progress": return theme.fg("accent", "●");
    case "pending": return theme.fg("muted", "○");
    case "dropped": return theme.fg("warning", "–");
  }
}

// Labels/reasons are plain data, not terminal escape sequences or new layout instructions.
function label(text: string): string {
  return text.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").replace(/\s+/g, " ").trim();
}

export class TodoWidget implements Component {
  private progress: TodoProgress | null = null;
  private expanded = false;
  private offset = 0;
  private maxOffset = 0;
  private stale = false;

  constructor(
    private readonly theme: Style,
    private readonly rows: () => number,
    private readonly refresh: () => void,
  ) {}

  update(progress: TodoProgress | null): void {
    // Late replayed receipts cannot replace a newer confirmed ledger.
    if (progress && this.progress && progress.revision < this.progress.revision) return;
    this.progress = progress;
    this.stale = false;
    this.refresh();
  }

  failed(): void {
    this.stale = true;
    this.refresh();
  }

  toggle(): void {
    this.expanded = !this.expanded;
    this.refresh();
  }

  scroll(delta: number): void {
    this.offset = Math.max(0, Math.min(this.maxOffset, this.offset + delta));
    this.refresh();
  }

  render(width: number): string[] {
    if (!this.progress && !this.stale) return [];
    const { theme, progress } = this;
    const steps = progress?.phases.flatMap((phase) => phase.steps) ?? [];
    const done = steps.filter((step) => step.status.state === "done").length;
    const dropped = steps.filter((step) => step.status.state === "dropped").length;
    const active = steps.find((step) => step.status.state === "in_progress");
    const next = steps.find((step) => step.status.state === "pending");
    const head = `${this.expanded ? "▾" : "▸"} Todo`;
    const counts = progress ? `  ${theme.fg("success", "✓")} ${done}/${steps.length}` +
      (dropped ? `  ${theme.fg("warning", "–")} ${dropped}` : "") : "";
    const status = this.stale ? theme.fg("warning", progress ? " · stale" : " · unavailable") : "";
    const current = active ?? next;
    const summary = !this.expanded && current ? `  ·  ${mark(current.status, theme)} ${label(current.label)}` : "";
    const header = truncateToWidth(`${theme.bold(head)}${counts}${status}${summary}`, width);
    if (!this.expanded || !progress) return [header];

    const body: string[] = [];
    const wrap = (text: string, indent: number) => {
      const padding = " ".repeat(indent);
      for (const line of new Text(text, 0, 0).render(Math.max(1, width - indent))) {
        body.push(truncateToWidth(padding + line, width));
      }
    };
    for (const phase of progress.phases) {
      body.push("");
      const completed = phase.steps.filter((step) => step.status.state === "done").length;
      wrap(theme.bold(`${label(phase.label)}  ✓ ${completed}/${phase.steps.length}`), 2);
      for (const step of phase.steps) {
        wrap(`${mark(step.status, theme)} ${label(step.label)} ${theme.fg("dim", `[${label(step.id)}]`)}`, 4);
        if (step.criteria.length) wrap(theme.fg("dim", `Criteria: ${step.criteria.map(label).join(", ")}`), 6);
        if (step.status.state === "dropped") wrap(theme.fg("warning", `Reason: ${label(step.status.reason)}`), 6);
      }
    }
    // Keep most of the terminal available for conversation and input, even in small panes.
    const height = Math.max(1, Math.min(16, Math.floor(this.rows() / 3)));
    this.maxOffset = Math.max(0, body.length - height);
    this.offset = Math.min(this.offset, this.maxOffset);
    const lines = body.slice(this.offset, this.offset + height);
    if (this.maxOffset) {
      lines.push(truncateToWidth(theme.fg("dim",
        `${this.offset + 1}–${this.offset + lines.length}/${body.length}  /todo up|down · click header to fold`), width));
    }
    return [header, ...lines];
  }

  handleMouse(event: TuiMouseEvent): { handled: boolean; render?: boolean } | undefined {
    if (event.type === "click" && event.button === "left" && event.y === 0) {
      this.toggle();
      return { handled: true };
    }
    if (event.type === "wheel" && this.expanded) {
      this.scroll(event.wheelDelta ?? 0);
      return { handled: true };
    }
    return undefined;
  }

  invalidate(): void {}
}
