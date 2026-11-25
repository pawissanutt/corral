import { ScrollView, matchesKey, truncateToWidth, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { BoardPlan, BoardTaskStatus } from "../build/types.ts";
import { renderBoard, type BoardAction } from "./build/board-view.ts";
import { keyCap, spinner } from "./frame.ts";
import { planStepDetail, STEP_GLYPH, WAIT_GLYPH, type PlanSource } from "./plan-detail.ts";
import { plainLabel, PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_SELECTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING, RESET } from "./panels-view.ts";

export type PlanDocument = { readonly phase: "off" | "planning" }
  | { readonly phase: "review" | "approved"; readonly title: string; readonly revision: number };
interface PlanPageData {
  readonly document: PlanDocument | undefined;
  readonly steps: PlanSource;
  readonly board?: { readonly tasks: readonly BoardTaskStatus[]; readonly proposed: BoardPlan | undefined; readonly unavailable: string | undefined };
}
type Target = { readonly kind: "document" | "step" | "board"; readonly key: string; readonly id?: string; readonly action?: BoardAction };
interface Hit { readonly target: Target; readonly from?: number; readonly to?: number }
const doc: Target = { kind: "document", key: "document" };
const boardTarget = (action: BoardAction, id?: string): Target => ({ kind: "board", key: `board:${action}:${id ?? ""}`, action, ...(id === undefined ? {} : { id }) });
const stepTarget = (id: string): Target => ({ kind: "step", key: `step:${id}`, id });

export function stepCount(source: PlanSource): string | undefined {
  const steps = source.progress?.phases.flatMap(phase => phase.steps);
  return steps && `${steps.filter(step => step.status.state === "done").length}/${steps.length}`;
}
function targets(data: PlanPageData): Target[] {
  return [...(data.document?.phase === "review" || data.document?.phase === "approved" ? [doc] : []),
    ...(data.steps.progress?.phases.flatMap(phase => phase.steps.map(step => stepTarget(step.id))) ?? []),
    ...(data.board?.proposed ? [boardTarget("review")] : []), ...(data.board?.tasks.map(task => boardTarget("details", task.id)) ?? [])];
}

/** Draw the three plan sources in reading order, preserving Board pointer targets. */
export function renderPlanPage(data: PlanPageData, key: string | undefined, width: number,
  options: { readonly short: boolean; readonly now: number; readonly focused: boolean }) {
  const rows = targets(data), selected = rows.find(target => target.key === key) ?? rows[0];
  const lines: string[] = [], hits = new Map<number, readonly Hit[]>();
  let selectedRow = -1;
  const push = (text: string, target?: Target, cap = false) => {
    const active = target !== undefined && target.key === selected?.key && !cap, base = (active ? PANEL_SELECTED : PANEL_BG) + PANEL_TEXT;
    if (target) hits.set(lines.length, [{ target }]);
    if (active) selectedRow = lines.length;
    const content = truncateToWidth((active ? (options.focused ? PANEL_BLUE : PANEL_MUTED) + "▌" : " ") + text, width, "…");
    lines.push(base + content.replaceAll(RESET, RESET + base) + " ".repeat(Math.max(0, width - visibleWidth(content))) + RESET);
  };
  const heading = (text: string, meta = "") => { if (lines.length && !options.short) push(""); push(`\x1b[1m${text}\x1b[22m${meta}`); };
  const document = data.document;
  if (document) {
    heading("Plan document");
    if (document.phase === "review") {
      push(` ${PANEL_WARNING}◆ ${plainLabel(document.title)} · revision ${document.revision} · waits for your review`, doc);
      push(` ${keyCap("⏎")} review`, doc, true);
    } else if (document.phase === "approved") push(` ${PANEL_SUCCESS}✓ ${plainLabel(document.title)} · revision ${document.revision} · approved`, doc);
    else push(` ${PANEL_MUTED}${document.phase === "planning" ? "Planning · not submitted yet" : "No plan document. /plan starts planning."}`);
  }
  const { progress, stale } = data.steps;
  const plan = progress?.plan ? `${plainLabel(progress.plan)} · ` : "";
  heading("Steps", progress ? `${PANEL_MUTED} · ${plan}r${progress.revision} · ${stepCount(data.steps)} done${stale ? `${PANEL_WARNING} · stale` : ""}` : "");
  if (!progress) push(` ${PANEL_MUTED}No steps yet.`);
  else {
    let running = 0;
    for (const phase of progress.phases) {
      push(` ${PANEL_MUTED}${plainLabel(phase.label)}`);
      for (const step of phase.steps) {
        const state = step.status.state, glyph = state === "in_progress" ? spinner(options.now, running++) : STEP_GLYPH[state];
        const color = state === "done" ? PANEL_SUCCESS : state === "in_progress" ? PANEL_BLUE : PANEL_MUTED;
        const label = state === "done" || state === "dropped" ? PANEL_MUTED : PANEL_TEXT;
        push(`   ${color}${glyph} ${label}${plainLabel(step.label)}`, stepTarget(step.id));
        for (const wait of planStepDetail(progress, step.id)?.waits ?? []) {
          const lead = wait.kind === "owner" ? `${PANEL_WARNING}${WAIT_GLYPH.owner} waits for you: ` : `${PANEL_MUTED}${WAIT_GLYPH.event} waits for `;
          push(`     ${lead}${wait.text}`);
        }
      }
    }
  }
  if (data.board) {
    heading("Board tasks");
    const { tasks, proposed, unavailable } = data.board, offset = lines.length;
    const board = renderBoard(tasks, proposed, selected?.action === "details" ? selected.id : null, width,
      { ...options, selectReview: selected?.action === "review", ...(unavailable === undefined ? {} : { unavailable }) });
    lines.push(...board.lines);
    for (const [row, items] of board.hits) hits.set(offset + row, items.map(({ action, id, ...range }) => ({ ...range, target: boardTarget(action ?? "details", id) })));
    if (board.selectedRow >= 0) selectedRow = offset + board.selectedRow;
  }
  return { lines, hits, selectedRow };
}

interface PlanPageHost {
  data(): PlanPageData; now(): number; short(): boolean; focused(): boolean;
  click(event: TuiMouseEvent): boolean; focus(): void;
  document(): void; ledger(step: string): void; board(action: BoardAction, id?: string): void;
}

/** One scroll and one keyed selection for all three sections. */
export class PlanPage {
  readonly scroll: ScrollView;
  private selected: string | undefined;
  private hits: ReadonlyMap<number, readonly Hit[]> = new Map();
  private reveal = false;
  constructor(private readonly host: PlanPageHost) {
    this.scroll = new ScrollView({ render: width => this.render(width), handleMouse: event => this.mouse(event), invalidate() {} },
      { primary: true, follow: "none", scrollbar: "auto" });
  }
  actions(): readonly string[] {
    const data = this.host.data(), rows = targets(data);
    return [...(rows.length ? ["select", "open"] : []), ...(data.board?.proposed ? ["review"] : []),
      ...(data.board && data.board.unavailable === undefined ? ["plan", "refresh"] : [])];
  }
  private act(target: Target): void {
    if (target.kind === "document") this.host.document();
    else if (target.kind === "step" && target.id !== undefined) this.host.ledger(target.id);
    else if (target.action) this.host.board(target.action, target.id);
  }
  handleKey(data: string): boolean {
    const rows = targets(this.host.data()), index = Math.max(0, rows.findIndex(row => row.key === this.selected));
    const delta = matchesKey(data, "up") || data === "k" ? -1 : matchesKey(data, "down") || data === "j" ? 1 : 0;
    if (delta && rows.length) { this.selected = rows[Math.min(rows.length - 1, Math.max(0, index + delta))]?.key; this.reveal = true; return true; }
    if (matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) {
      this.scroll.scrollBy((matchesKey(data, "pageUp") ? -1 : 1) * Math.max(1, this.scroll.viewportHeight - 2)); return true;
    }
    const action = data === "p" ? "review" : data === "n" ? "plan" : data === "r" ? "refresh" : undefined;
    const target = matchesKey(data, "enter") ? rows[index] : action && this.actions().includes(action) ? boardTarget(action) : undefined;
    if (!target) return false;
    this.act(target); return true;
  }
  private render(width: number): string[] {
    const frame = renderPlanPage(this.host.data(), this.selected, width, { now: this.host.now(), short: this.host.short(), focused: this.host.focused() });
    this.hits = frame.hits;
    const height = this.scroll.viewportHeight, top = this.scroll.scrollTop, row = frame.selectedRow;
    if (this.reveal && row >= 0 && height > 0) {
      this.reveal = false;
      const target = row < top ? row : row >= top + height ? row + 1 - height : top;
      if (target !== top) { this.scroll.updateLayout(frame.lines.length, height, () => {}); this.scroll.scrollTo(target, { disableFollow: true }); }
    }
    return frame.lines;
  }
  private mouse(event: TuiMouseEvent): { readonly handled: boolean } | undefined {
    if (!this.host.click(event)) return undefined;
    this.host.focus();
    const hits = this.hits.get(event.y) ?? [];
    const hit = hits.find(item => item.from !== undefined && item.to !== undefined && event.x >= item.from && event.x < item.to)
      ?? hits.find(item => item.from === undefined);
    if (hit) {
      const { target } = hit, rows = targets(this.host.data()), shown = rows.find(row => row.key === this.selected) ?? rows[0];
      if (rows.some(row => row.key === target.key)) this.selected = target.key;
      if (target.action !== "details" || hit.from !== undefined || shown?.key === target.key) this.act(target);
    }
    return { handled: true };
  }
}
