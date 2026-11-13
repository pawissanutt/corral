// One agent's conversation as the mock's `childHTML` draws it: the prompt as the transcript's own user bubble
// with this conversation's label in place of `you`, the replies through Pi's AssistantMessageComponent, and each
// tool call on one line (`✓ read src/a.ts`). A tool line opens into Pi's native ToolExecutionComponent card by
// pointer, and every tool does while tools are expanded. Only bounded, sanitized display data reaches the renderers.
import {
  AssistantMessageComponent, ToolExecutionComponent, UserMessageComponent, getMarkdownTheme, type MarkdownTransformer, type ToolRenderers,
} from "@earendil-works/pi-coding-agent";
import {
  stripTerminalSequences, truncateToWidth, visibleWidth, wrapTextWithAnsi, type Component, type MarkdownTheme, type TUI, type TuiMouseEvent,
  type TuiMouseEventResult,
} from "@earendil-works/pi-tui";
import type { HelperMessage } from "../extensions/subagent/inspection.ts";
import type { ConversationSource } from "./agents-model.ts";
import { spinner } from "./frame.ts";
import { userLabel } from "./panels-theme.ts";
import { plainLabel, PANEL_BLUE, PANEL_BUBBLE, PANEL_ERROR, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT } from "./panels-view.ts";

export interface HelperChatStyle {
  readonly outputPad: 0 | 1;
  readonly markdownTheme: MarkdownTheme;
  readonly markdownTransformers: readonly MarkdownTransformer[];
  readonly renderers: (name: string) => ToolRenderers | undefined;
}
/** The prompt bubble's label (`brief from parent`) and the muted lines under the prompt inside it. */
export interface PromptBubble { readonly label: string; readonly notes?: readonly string[] }

const RESET = "\x1b[0m";
/** The widest a bubble grows, as the mock's `.msg`. */
const BUBBLE = 96;

export function helperText(text: string): string {
  return stripTerminalSequences(text).replace(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g, " ");
}
function safeValue(value: unknown, depth = 0): unknown {
  if (depth > 16) return "[nested value omitted]";
  if (typeof value === "string") return helperText(value);
  if (Array.isArray(value)) return value.map(item => safeValue(item, depth + 1));
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [helperText(key), safeValue(item, depth + 1)]));
  }
  return value;
}
function toolArgs(text: string | undefined): { readonly value: unknown; readonly complete: boolean } {
  if (text === undefined) return { value: {}, complete: false };
  try {
    const parsed: unknown = JSON.parse(text);
    return parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)
      ? { value: safeValue(parsed), complete: true } : { value: { preview: helperText(text) }, complete: false };
  } catch { return { value: { preview: helperText(text), note: "arguments truncated or unavailable" }, complete: false }; }
}
function argumentRecord(text: string | undefined): Readonly<Record<string, unknown>> | undefined {
  if (text === undefined) return undefined;
  try {
    const value: unknown = JSON.parse(text);
    return value !== null && typeof value === "object" && !Array.isArray(value) ? Object.fromEntries(Object.entries(value)) : undefined;
  } catch { return undefined; }
}
/** What a tool call touches, from its arguments: `src/a.ts:10-20` for read, the pattern and path for grep, otherwise the command, path or query. */
export function toolTarget(message: HelperMessage): string {
  const name = plainLabel(helperText(message.tool ?? "tool")), args = argumentRecord(message.arguments);
  const field = (key: string) => { const value = args?.[key]; return typeof value === "string" || typeof value === "number" ? plainLabel(helperText(String(value))) : undefined; };
  const offset = Number(args?.["offset"]), limit = Number(args?.["limit"]);
  const range = Number.isInteger(offset) && offset > 0 ? `:${offset}${Number.isInteger(limit) && limit > 0 ? `-${offset + limit - 1}` : ""}` : "";
  const summary = !args ? plainLabel(helperText(message.arguments ?? "")).slice(0, 80)
    : name === "read" ? (field("path") ?? "") + range
    : name === "grep" ? [field("pattern") === undefined ? "" : `"${field("pattern")}"`, field("path"), field("glob")].filter(Boolean).join(" ")
    : field("command") ?? field("path") ?? field("pattern") ?? field("query") ?? field("url")
      ?? Object.values(args).filter((value): value is string => typeof value === "string").map(value => plainLabel(helperText(value))).join(" ");
  return summary.slice(0, 160).trim();
}
/** One plain line for a tool call: its name, then the argument that says what it touches (`read src/a.ts:10-20`). */
export function toolSummary(message: HelperMessage): string {
  return (plainLabel(helperText(message.tool ?? "tool")) + " " + toolTarget(message)).trim();
}

/**
 * The last transformer of a prompt bubble's chain: the owner's `you` label, put there by `userLabel` earlier in the
 * chain (the host registers it wrapped, so it is found by its output, not by reference) or added here when the
 * chain lacks it, becomes `label`. Placement and colours stay `userLabel`'s, so a prompt bubble is the transcript's
 * user bubble apart from its label. Prompt text is sanitized, so it can never begin with the label's colour code.
 */
export function promptLabel(label: string): MarkdownTransformer {
  return (markdown, context) => {
    const you = PANEL_MUTED + "you", labelled = markdown.startsWith(you) ? markdown : userLabel(markdown, context);
    return labelled.startsWith(you) ? PANEL_MUTED + label + labelled.slice(you.length) : labelled;
  };
}
/** Rows on the bubble surface in the transcript's user-bubble geometry: one cell in, at most 96 cells wide, `pad` cells inside each side. */
function bubbleRows(lines: readonly string[], width: number, pad: number): string[] {
  const outer = Math.max(1, Math.min(width - 1, BUBBLE)), inner = Math.max(1, outer - 2 * pad);
  return lines.flatMap(line => wrapTextWithAnsi(line, inner)).map(row => {
    const kept = truncateToWidth(row, inner, "").replaceAll(RESET, RESET + PANEL_BUBBLE + PANEL_TEXT);
    return " " + PANEL_BUBBLE + PANEL_TEXT + " ".repeat(pad) + kept + PANEL_BUBBLE + " ".repeat(Math.max(0, inner - visibleWidth(kept)) + pad) + RESET;
  });
}
/**
 * A plain-text message bubble in the transcript's user-bubble geometry: `label` in muted, then `text`, then `notes`
 * in muted, with one cell of padding each side. `label` may carry its own colours.
 */
export function bubble(label: string, text: string, width: number, notes: readonly string[] = []): string[] {
  return bubbleRows([PANEL_MUTED + label + " " + PANEL_TEXT + text, ...notes.map(note => PANEL_MUTED + note)], width, 1);
}
/** One tool call on one line: its state (the spinner while the agent runs it), its name in accent and what it touches. */
function toolLine(message: HelperMessage, live: boolean, now: number, phase: number, width: number): string {
  const state = message.phase === "completed" ? PANEL_SUCCESS + "✓" : message.phase === "error" ? PANEL_ERROR + "✗"
    : live ? PANEL_BLUE + spinner(now, phase) : PANEL_MUTED + "○";
  const target = toolTarget(message);
  return truncateToWidth(" " + state + " " + PANEL_BLUE + plainLabel(helperText(message.tool ?? "tool")) + (target ? " " + PANEL_TEXT + target : "") + PANEL_TEXT,
    width, "…") + RESET;
}

interface Cached { readonly component: AssistantMessageComponent | ToolExecutionComponent; key: string; readonly argsComplete?: boolean }
/** A rendered stretch of rows: a native component, or the one-line form of the tool call `tool`. */
interface Hit { readonly start: number; readonly end: number; readonly component?: Component; readonly tool?: string }

export class HelperChat {
  private request: string | undefined;
  private readonly cached = new Map<string, Cached>();
  /** Tool calls opened into their native card by pointer while tools are collapsed. */
  private readonly opened = new Set<string>();
  private hits: Hit[] = [];
  private styleKey = "";
  private prompt: UserMessageComponent | undefined;
  private expanded = false;
  constructor(private readonly tui: () => TUI | undefined, private readonly cwd: string,
    private readonly style?: () => HelperChatStyle) {}
  /**
   * The conversation rows for `width`: the prompt bubble (labelled `bubble.label`, default `prompt`) with its notes
   * inside, then replies and tool lines in order, a blank row before each run of tool lines that follows the
   * prompt or a reply.
   */
  renderHelper(helper: ConversationSource, width: number,
    options: { readonly bubble?: PromptBubble; readonly now?: number } = {}): string[] {
    const style = this.style?.() ?? { outputPad: 1, markdownTheme: getMarkdownTheme(), markdownTransformers: [], renderers: () => undefined };
    const label = options.bubble?.label ?? "prompt", transformers = style.markdownTransformers;
    // The host hands out fresh wrapper functions with every presentation, so the chain is keyed by its length.
    const styleKey = `${style.outputPad}/${style.markdownTheme.codeBlockIndent ?? ""}/${transformers.length}/${label}`;
    if (this.request !== helper.request) { this.clear(); this.request = helper.request; }
    if (this.styleKey !== styleKey) {
      this.prompt = undefined; this.cached.clear(); this.styleKey = styleKey;
    }
    // The prompt's label takes the place of the owner's `you` at the end of the same transformer chain.
    this.prompt ??= new UserMessageComponent(helperText(helper.prompt), style.markdownTheme, style.outputPad, [...transformers, promptLabel(label)]);
    const rows: string[] = [];
    this.hits = [];
    const append = (component: Component, tool?: string) => {
      const lines = component.render(width);
      this.hits.push({ start: rows.length, end: rows.length + lines.length, component, ...(tool === undefined ? {} : { tool }) }); rows.push(...lines);
    };
    append(this.prompt);
    rows.push(...bubbleRows((options.bubble?.notes ?? []).map(note => PANEL_MUTED + note), width, style.outputPad));
    const live = helper.phase === "running" || helper.phase === "needs", now = options.now ?? Date.now();
    let lastLine = false;
    const retained = new Set<string>();
    const messages: readonly HelperMessage[] = helper.result !== undefined && !helper.messages.some(message => message.kind === "assistant" && (message.textBlocks?.join("") ?? message.text).trim() === helper.result)
      ? [...helper.messages, { id: "returned-result", kind: "assistant", text: helper.result, stopReason: "pending" }] : helper.messages;
    messages.forEach((message, index) => {
      retained.add(message.id);
      const key = JSON.stringify([message.text, message.textBlocks, message.arguments, message.phase, message.stopReason, message.errorMessage, message.hasToolCalls]);
      let cached = this.cached.get(message.id);
      if (message.kind === "assistant") {
        const component = cached?.component instanceof AssistantMessageComponent ? cached.component
          : new AssistantMessageComponent(undefined, true, style.markdownTheme, "", style.outputPad, transformers);
        if (!cached || cached.key !== key) component.updateContent({ content: (message.textBlocks ?? [message.text]).map(text => ({ type: "text", text: helperText(text) })),
          stopReason: message.stopReason ?? "pending", errorMessage: message.errorMessage === undefined ? undefined : helperText(message.errorMessage), hasToolCalls: message.hasToolCalls }, helper.phase === "running" && message.stopReason === undefined);
        cached = { component, key };
        this.cached.set(message.id, cached); append(component); lastLine = false;
        return;
      }
      // A headless host has no interactive tool-renderer context, so its tools stay one line.
      const tui = this.expanded || this.opened.has(message.id) ? this.tui() : undefined;
      if (!tui) {
        if (!lastLine) rows.push("");
        this.hits.push({ start: rows.length, end: rows.length + 1, tool: message.id });
        rows.push(toolLine(message, live, now, index, width)); lastLine = true;
        return;
      }
      const args = toolArgs(message.arguments);
      const reused = cached?.component instanceof ToolExecutionComponent && cached.argsComplete === args.complete;
      const component = reused && cached?.component instanceof ToolExecutionComponent ? cached.component
        : new ToolExecutionComponent(helperText(message.tool ?? "unavailable"), message.id, args.value,
          { showImages: false, outputPad: style.outputPad }, args.complete ? style.renderers(message.tool ?? "") : undefined, tui, this.cwd);
      if (!reused) component.setExpanded(true);
      if (!cached || cached.key !== key || cached.argsComplete !== args.complete) {
        component.updateArgs(args.value);
        if (args.complete) component.setArgsComplete();
        if (message.phase !== "requested") component.markExecutionStarted();
        if (message.phase === "completed" || message.phase === "error") component.updateResult({
          content: [{ type: "text", text: helperText(message.text) }], isError: message.phase === "error",
        });
      }
      cached = { component, key, argsComplete: args.complete };
      this.cached.set(message.id, cached); append(component, message.id); lastLine = false;
    });
    for (const id of this.cached.keys()) if (!retained.has(id)) this.cached.delete(id);
    for (const id of this.opened) if (!retained.has(id)) this.opened.delete(id);
    return rows;
  }
  /** A click on a tool line opens its native card; a click that the open card handles folds it back to its line. */
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    const hit = this.hits.find(item => event.y >= item.start && event.y < item.end);
    if (!hit) return undefined;
    if (!hit.component) {
      if (hit.tool === undefined || event.type !== "click" || event.button !== "left") return undefined;
      this.opened.add(hit.tool);
      const cached = this.cached.get(hit.tool)?.component;
      if (cached instanceof ToolExecutionComponent) cached.setExpanded(true);
      return { handled: true };
    }
    const result = hit.component.handleMouse?.({ ...event, y: event.y - hit.start, height: hit.end - hit.start });
    if (result?.handled && hit.tool !== undefined && !this.expanded) this.opened.delete(hit.tool);
    return result;
  }
  /** Every tool as its expanded native card, or every tool back on one line. */
  expandTools(expanded: boolean): void {
    this.expanded = expanded;
    this.opened.clear();
    for (const item of this.cached.values()) if (item.component instanceof ToolExecutionComponent) item.component.setExpanded(true);
  }
  invalidate(): void {
    this.prompt?.invalidate();
    for (const item of this.cached.values()) item.component.invalidate();
  }
  clear(): void { this.prompt = undefined; this.cached.clear(); this.opened.clear(); this.hits = []; this.request = undefined; this.styleKey = ""; }
}
