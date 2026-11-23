// One agent's conversation as the mock's `childHTML` draws it: the prompt as the transcript's own user bubble
// with this conversation's label in place of `you`, the replies through Pi's AssistantMessageComponent, and each
// tool call on one row drawn by the same presenters as the main transcript (`✓ read src/a.ts · 2 lines`). A tool
// row opens into Pi's native ToolExecutionComponent card by pointer, and every tool does while tools are expanded.
// Only bounded, sanitized display data reaches the renderers.
import { parseStreamingJson } from "@earendil-works/pi-ai";
import {
  AssistantMessageComponent, ToolExecutionComponent, UserMessageComponent, getMarkdownTheme, type MarkdownTransformer, type ToolRenderers,
} from "@earendil-works/pi-coding-agent";
import {
  stripTerminalSequences, truncateToWidth, visibleWidth, wrapTextWithAnsi, type Component, type MarkdownTheme, type SelectionRow, type TUI,
  type TuiMouseEvent, type TuiMouseEventResult,
} from "@earendil-works/pi-tui";
import type { HelperMessage } from "../extensions/subagent/inspection.ts";
import type { AgentMail, ConversationSource } from "./agents-model.ts";
import { userLabel } from "./panels-theme.ts";
import { plainLabel, PANEL_BUBBLE, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";
import { selectionIn } from "./selection.ts";
import { presenterFor, type ToolRenderDeps } from "./tool-rendering.ts";
import { baseState, firstLine, presentCall, resultText, rowLine, type CallInput, type Presenter, type RowModel } from "./tool-row.ts";

export interface HelperChatStyle {
  readonly outputPad: 0 | 1;
  readonly markdownTheme: MarkdownTheme;
  readonly markdownTransformers: readonly MarkdownTransformer[];
  readonly renderers: (name: string) => ToolRenderers | undefined;
}
/** The prompt bubble's label (`brief from parent`) and the muted lines under the prompt inside it. */
export interface PromptBubble { readonly label: string; readonly notes?: readonly string[] }
/**
 * A bubble among the replies: an owner's steer or a message between agents, with when it was sent (without a time it
 * is drawn after the replies). `label` replaces the conversation's steer label; `notes` sit under the text.
 */
export interface Steer { readonly text: string; readonly at?: number; readonly label?: string; readonly notes?: readonly string[] }
/** The bubbles placed among one conversation's replies and the label of those without their own (`you → picker-ui`). */
export interface Steers { readonly label: string; readonly items: readonly Steer[] }

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
/**
 * A tool message's arguments for its native card. `native`: an object the tool's renderers can draw, complete or
 * still streaming; `generic`: retention clipped the JSON, so only the generic card may show the preview.
 */
function toolArgs(message: HelperMessage): { readonly value: Record<string, unknown>; readonly complete: boolean; readonly mode: "native" | "generic" } {
  const text = message.arguments;
  if (text === undefined) return { value: {}, complete: false, mode: "native" };
  if (message.argsComplete === false) return { value: record(safeValue(parseStreamingJson(text))), complete: false, mode: "native" };
  try {
    const parsed: unknown = JSON.parse(text);
    if (parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)) return { value: record(safeValue(parsed)), complete: true, mode: "native" };
    return { value: { preview: helperText(text) }, complete: false, mode: "generic" };
  } catch { return { value: { preview: helperText(text), note: "arguments truncated or unavailable" }, complete: false, mode: "generic" }; }
}
function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? Object.fromEntries(Object.entries(value)) : {};
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
/**
 * Messages between agents as bubbles among the replies, each labelled `<from> → <to>` with every side drawn by `name`
 * (an agent's id in its hue; `hue` is undefined for `parent`, `owner`, `all` and other sessions' keys). A message the
 * recipient read in its opening prompt, answered from its final report or never received says so under its text.
 */
export function mailSteers(mail: readonly AgentMail[], name: (id: string, hue: number | undefined) => string): Steer[] {
  return mail.map(message => {
    const note = message.delivery === "queued" ? ["queued · read when it starts"]
      : message.delivery === "answered_from_report" ? ["it had finished · answered from its final report"]
      : message.delivery === "undeliverable" ? [PANEL_WARNING + "! not delivered · no live recipient"] : [];
    return { text: helperText(message.text), at: message.at, notes: note,
      label: name(helperText(message.from), message.fromHue) + PANEL_MUTED + " → " + name(helperText(message.to), message.toHue) };
  });
}
/**
 * What the retention limit took from a conversation: the count of earlier messages dropped when it is known
 * without implying that the source recording has been truncated.
 */
export function retentionNote(source: { readonly dropped?: number }): string {
  const dropped = source.dropped ?? 0;
  return dropped > 0 ? `${dropped} earlier ${dropped === 1 ? "message is" : "messages are"} not shown in this live preview`
    : "Some content is not shown in this live preview";
}
/** The call-site hint of a Code Mode script: the tools it calls, in order of first use (`read, bash`). */
function callSites(code: string): string {
  const names = [...code.matchAll(/\btools\.([A-Za-z_$][\w$]*)\s*\(/g)].map(match => match[1]!);
  return [...new Set(names)].join(", ");
}
/** A child's tool message as the presenters' call: its arguments as far as they streamed, and its result so far. */
function helperCall(message: HelperMessage, cwd: string): CallInput {
  const args = toolArgs(message), name = helperText(message.tool ?? "tool");
  const text = helperText(message.text);
  const settled = message.phase === "completed" || message.phase === "error";
  const partial = !settled && message.partial === true && text !== "";
  // Retention may clip a finished call's JSON: the row still names what the call touched, from the readable prefix.
  const value = args.mode === "generic" && message.arguments !== undefined ? record(safeValue(parseStreamingJson(message.arguments))) : args.value;
  return {
    name, args: value, argsComplete: args.complete || (args.mode === "generic" && message.argsComplete !== false),
    executionStarted: message.phase !== undefined && message.phase !== "requested",
    ...(settled || partial ? { result: { content: [{ type: "text" as const, text }], details: undefined } } : {}),
    partial, isError: message.phase === "error", expanded: false, cwd, toolCallId: message.id,
  };
}
/**
 * What a child's rows may read of host state: only the session cwd. A child's jobs, Code Mode cells and todo ledger
 * are its own, not this host's, so the presenters draw them from the call alone.
 */
function childDeps(cwd: string): ToolRenderDeps {
  return { cwd, ticker: () => undefined, job: () => undefined, code: () => undefined, ledger: () => undefined, corralSpecs: [] };
}
/** The row of a child's tool call: the same presenter as the main transcript; a Code Mode cell reads as the tools it calls. */
function helperRow(message: HelperMessage, live: boolean, cwd: string): RowModel {
  const call = helperCall(message, cwd);
  const present: Presenter = call.name === "codemode" ? codeRow : presenterFor(call.name, childDeps(cwd));
  const model = presentCall(present, call);
  // A conversation that stopped leaves its unfinished calls waiting, never spinning.
  return !live && (model.state === "writing" || model.state === "running") ? { ...model, state: "waiting" } : model;
}
/** A child's Code Mode call as `code · read, bash`: the tools its script calls, never the script. */
function codeRow(call: CallInput): RowModel {
  const sites = callSites(typeof call.args.code === "string" ? call.args.code : ""), state = baseState(call);
  const reason = state === "failed" ? firstLine(resultText(call.result)) : "";
  return { state, verb: "code", ...(reason ? { outcome: reason } : sites ? { outcome: sites } : {}) };
}

interface Cached {
  readonly component: AssistantMessageComponent | ToolExecutionComponent; key: string;
  readonly mode?: "native" | "generic"; readonly argsComplete?: boolean;
}
/** A rendered stretch of rows: a native component, or the one-line form of the tool call `tool`. */
interface Hit { readonly start: number; readonly end: number; readonly component?: Component; readonly tool?: string }

export class HelperChat {
  private request: string | undefined;
  private readonly cached = new Map<string, Cached>();
  /** Tool calls opened into their native card by pointer while tools are collapsed. */
  private readonly opened = new Set<string>();
  private hits: Hit[] = [];
  private hitsWidth = 0;
  private styleKey = "";
  private prompt: UserMessageComponent | undefined;
  private expanded = false;
  constructor(private readonly tui: () => TUI | undefined, private readonly cwd: string,
    private readonly style?: () => HelperChatStyle) {}
  /**
   * The conversation rows for `width`: the prompt bubble (labelled `bubble.label`, default `prompt`) with its notes
   * inside, the retention note when the limit dropped or clipped content, then replies and tool lines in order, a
   * blank row before each run of tool lines that follows the prompt or a reply. Each steer sits before the first
   * message that started after it was sent; steers sent after every message, and steers without a time, follow.
   */
  renderHelper(helper: ConversationSource, width: number,
    options: { readonly bubble?: PromptBubble; readonly now?: number; readonly steers?: Steers } = {}): string[] {
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
    this.hits = []; this.hitsWidth = width;
    const append = (component: Component, tool?: string) => {
      const lines = component.render(width);
      this.hits.push({ start: rows.length, end: rows.length + lines.length, component, ...(tool === undefined ? {} : { tool }) }); rows.push(...lines);
    };
    append(this.prompt);
    rows.push(...bubbleRows((options.bubble?.notes ?? []).map(note => PANEL_MUTED + note), width, style.outputPad));
    // The note stands where the dropped messages were: right under the prompt, wrapped under its text, not its glyph.
    if (helper.truncated) rows.push("", ...wrapTextWithAnsi(retentionNote(helper), Math.max(1, width - 3))
      .map((line, index) => " " + PANEL_WARNING + (index === 0 ? "! " : "  ") + line + RESET));
    const live = helper.phase === "running" || helper.phase === "needs", now = options.now ?? Date.now();
    let lastLine = false;
    const timed = (options.steers?.items ?? []).filter(steer => steer.at !== undefined).sort((a, b) => (a.at ?? 0) - (b.at ?? 0));
    let steered = 0;
    const steer = (item: Steer) => {
      if (options.steers) rows.push("", ...bubble(item.label ?? options.steers.label, item.text, width, item.notes));
      lastLine = false;
    };
    const retained = new Set<string>();
    const messages: readonly HelperMessage[] = helper.result !== undefined && !helper.messages.some(message => message.kind === "assistant" && (message.textBlocks?.join("") ?? message.text).trim() === helper.result)
      ? [...helper.messages, { id: "returned-result", kind: "assistant", text: helper.result, stopReason: "pending" }] : helper.messages;
    messages.forEach(message => {
      const started = message.at;
      if (started !== undefined) while (steered < timed.length && (timed[steered]!.at ?? 0) <= started) steer(timed[steered++]!);
      retained.add(message.id);
      const key = JSON.stringify([message.text, message.textBlocks, message.arguments, message.argsComplete, message.partial, message.phase, message.stopReason, message.errorMessage, message.hasToolCalls]);
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
        rows.push(" " + rowLine(helperRow(message, live, this.cwd), Math.max(1, width - 1), now)); lastLine = true;
        return;
      }
      // One card per call id: it is rebuilt only when the arguments stop being readable, never because they completed.
      const args = toolArgs(message);
      const reused = cached?.component instanceof ToolExecutionComponent && cached.mode === args.mode;
      const component = reused && cached?.component instanceof ToolExecutionComponent ? cached.component
        : new ToolExecutionComponent(helperText(message.tool ?? "unavailable"), message.id, args.value,
          { showImages: false, outputPad: style.outputPad }, args.mode === "native" ? style.renderers(message.tool ?? "") : undefined, tui, this.cwd);
      if (!reused) component.setExpanded(true);
      if (!cached || cached.key !== key || !reused) {
        component.updateArgs(args.value);
        if (args.complete && !(reused && cached?.argsComplete)) component.setArgsComplete();
        if (message.phase !== undefined && message.phase !== "requested") component.markExecutionStarted();
        if (message.phase === "completed" || message.phase === "error") component.updateResult({
          content: [{ type: "text", text: helperText(message.text) }], isError: message.phase === "error",
        });
        else if (message.partial === true && message.text !== "") component.updateResult({ content: [{ type: "text", text: helperText(message.text) }], isError: false }, true);
      }
      cached = { component, key, mode: args.mode, argsComplete: args.complete };
      this.cached.set(message.id, cached); append(component, message.id); lastLine = false;
    });
    while (steered < timed.length) steer(timed[steered++]!);
    for (const item of options.steers?.items ?? []) if (item.at === undefined) steer(item);
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
  /** Semantic selection inside the prompt, reply or tool card drawn at `y` by the last `renderHelper`. */
  selectionRows(width: number, y: number): readonly SelectionRow[] | undefined {
    if (width !== this.hitsWidth) return undefined;
    const hit = this.hits.find(item => y >= item.start && y < item.end);
    return hit?.component ? selectionIn(hit.component, width, y, hit.start, 0) : undefined;
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
