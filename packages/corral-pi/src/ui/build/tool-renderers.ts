// Transcript renderers for the build tools. `task` draws the live slice card and, once the call returns, its
// totals; `build_finish` draws the result card with live exit progress; `board_plan` draws one plan line and
// whether the owner still has to confirm it. Live cards redraw through `BuildCardSource.watch` until they settle.
import type { AgentToolResult, ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { BoardPlan, BoardTaskPlan, ChildSnapshot, ChildSpec } from "../../build/types.ts";
import { plainLabel, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING } from "../panels-view.ts";
import { planEdges } from "./build-dialogs.ts";
import { BuildCard, finishLive, resultCard } from "./build-entries.ts";
import type { BuildCardSource } from "./contract.ts";
import {
  BOLD, cardLine, cardWrap, firstLine, taskCardLines, taskCardRows, taskRowsSettled, taskTotals, totalsText, UNBOLD,
} from "./task-card.ts";

type Renderers = Pick<ToolDefinition, "renderCall" | "renderResult">;
type RenderContext = Parameters<NonNullable<ToolDefinition["renderCall"]>>[2];
type Source = () => BuildCardSource | undefined;
type TaskArgs = { readonly agents: readonly ChildSpec[] };

/** Kept in Pi's per-call renderer state, shared by the call and result renders of one tool call. */
interface CardState {
  release?: () => void;
  /** The watch was released for good: the card has settled. */
  ended?: boolean;
  /** The final result arrived. */
  result?: boolean;
  /** `details.children` of the `task` result. */
  settled?: readonly ChildSnapshot[];
}
/** Pi starts every tool row with an empty object as renderer state and only these renderers write it. */
function cardState(context: RenderContext): CardState {
  return context.state as CardState;
}

/** Registers one watch per tool call while the card is live, and releases it once `live` turns false. */
function track(source: Source, context: RenderContext, live: boolean): void {
  const state = cardState(context);
  if (state.ended) return;
  if (!live) { state.release?.(); state.release = undefined; state.ended = true; return; }
  if (state.release) return;
  const cards = source();
  if (cards) state.release = cards.watch(context.toolCallId, context.invalidate);
}
function resultText(result: AgentToolResult<unknown>): string {
  return result.content.flatMap(part => part.type === "text" ? [part.text] : []).join("\n");
}
function errorCard(result: AgentToolResult<unknown>): BuildCard<never> {
  const text = firstLine(resultText(result)) || "The tool failed.";
  return new BuildCard<never>(width => ({ lines: cardWrap(PANEL_ERROR + "✗ " + text, "", width) }));
}

function taskRenderers(source: Source): Renderers {
  const rowsOf = (args: unknown, context: RenderContext) => {
    const cards = source();
    // taskCardRows keeps only argument entries that carry an id, so streaming or malformed arguments are safe here.
    const spec = args as TaskArgs;
    return taskCardRows(spec, context.toolCallId, cards?.state()?.fleet, cardState(context).settled, cards?.now() ?? Date.now());
  };
  return {
    renderCall: (args, _theme, context) => {
      const expanded = context.expanded;
      track(source, context, !(cardState(context).result && taskRowsSettled(rowsOf(args, context))));
      return new BuildCard<{ readonly id: string; readonly needs: boolean }>(width => {
        const cards = source();
        return taskCardLines(rowsOf(args, context), width, { expanded, color: hue => cards?.color(hue) ?? "" });
      }, hit => {
        const cards = source();
        if (hit.needs) cards?.question(hit.id);
        else cards?.openChild(hit.id);
      });
    },
    renderResult: (result, options, _theme, context) => {
      const state = cardState(context), details: unknown = result.details;
      if (typeof details === "object" && details !== null && "children" in details && Array.isArray(details.children)) {
        state.settled = details.children as ChildSnapshot[];   // the `task` tool returns its ChildSnapshot list here
      }
      if (!options.isPartial) state.result = true;
      track(source, context, !(state.result && taskRowsSettled(rowsOf(context.args, context))));
      if (!state.settled?.length && context.isError) return errorCard(result);
      return new BuildCard(width => {
        const live = source()?.state()?.fleet.children.filter(child => child.call === context.toolCallId) ?? [];
        const children = live.length ? live : state.settled ?? [];
        return { lines: children.length ? cardWrap(totalsText(taskTotals(children)), "", width) : [] };
      });
    },
  };
}

function finishRenderers(source: Source): Renderers {
  return {
    renderCall: (args: unknown, _theme, context) => {
      const summary = typeof args === "object" && args !== null && "summary" in args ? args.summary : undefined;
      track(source, context, !cardState(context).result || finishLive(source(), context.toolCallId));
      return resultCard(source, context.toolCallId, typeof summary === "string" ? summary : undefined);
    },
    renderResult: (result, options, _theme, context) => {
      const state = cardState(context);
      if (!options.isPartial) state.result = true;
      track(source, context, !state.result || finishLive(source(), context.toolCallId));
      return context.isError ? errorCard(result) : new BuildCard(() => ({ lines: [] }));
    },
  };
}

/** The plan as far as its arguments have streamed: only tasks with a key and a dependency list count. */
function planOf(args: unknown): BoardPlan {
  if (typeof args !== "object" || args === null) return { title: "", why: "", tasks: [] };
  const title = "title" in args && typeof args.title === "string" ? args.title : "";
  const list = "tasks" in args && Array.isArray(args.tasks) ? args.tasks as unknown[] : [];
  const tasks = list.flatMap((task): BoardTaskPlan[] => {
    if (typeof task !== "object" || task === null) return [];
    const key: unknown = "key" in task ? task.key : undefined, depends: unknown = "depends" in task ? task.depends : undefined;
    const name: unknown = "title" in task ? task.title : undefined;
    if (typeof key !== "string" || !Array.isArray(depends)) return [];
    return [{ key, title: typeof name === "string" ? name : "", body: "", depends: depends.filter((dep): dep is string => typeof dep === "string") }];
  });
  return { title, why: "", tasks };
}
/** The proposed plan is this call's when its title and task keys match. */
function waiting(cards: BuildCardSource | undefined, plan: BoardPlan): boolean {
  const proposed = cards?.proposed?.();
  return proposed !== undefined && proposed.title === plan.title && proposed.tasks.length === plan.tasks.length
    && proposed.tasks.every((task, index) => task.key === plan.tasks[index]?.key);
}

function planRenderers(source: Source): Renderers {
  return {
    renderCall: (args: unknown) => {
      const plan = planOf(args), edges = planEdges(plan);
      const tasks = `${plan.tasks.length} ${plan.tasks.length === 1 ? "task" : "tasks"}`;
      const text = BOLD + PANEL_BLUE + "board plan" + UNBOLD + (plan.title ? PANEL_MUTED + " · " + PANEL_TEXT + plainLabel(plan.title) : "")
        + PANEL_MUTED + " · " + PANEL_TEXT + `${tasks}, ${edges} dependency ${edges === 1 ? "edge" : "edges"}`;
      return new BuildCard(width => ({ lines: [cardLine(text, width)] }));
    },
    renderResult: (result, _options, _theme, context) => {
      if (context.isError) return errorCard(result);
      const plan = planOf(context.args), text = firstLine(resultText(result));
      track(source, context, waiting(source(), plan));
      return new BuildCard<true>(width => waiting(source(), plan)
        ? { lines: [cardLine(PANEL_WARNING + "◆ waiting for your confirmation", width)], hits: new Map([[0, true as const]]) }
        : { lines: text ? cardWrap(text, PANEL_MUTED, width) : [] }, () => source()?.plan());
    },
  };
}

/** `task`, `build_finish` and `board_plan`; undefined for any other name. */
export function buildToolRenderers(name: string, source: () => BuildCardSource | undefined): Renderers | undefined {
  switch (name) {
    case "task": return taskRenderers(source);
    case "build_finish": return finishRenderers(source);
    case "board_plan": return planRenderers(source);
    default: return undefined;
  }
}
