import {
  AssistantMessageComponent, UserMessageComponent, ToolExecutionComponent, getMarkdownTheme,
  type MarkdownTransformer, type ToolRenderers,
} from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, type Component, type MarkdownTheme, type TUI, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { HelperMessage } from "../extensions/subagent/inspection.ts";
import type { ConversationSource } from "./agents-model.ts";

export interface HelperChatStyle {
  readonly outputPad: 0 | 1;
  readonly markdownTheme: MarkdownTheme;
  readonly markdownTransformers: readonly MarkdownTransformer[];
  readonly renderers: (name: string) => ToolRenderers | undefined;
}
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
interface Cached { readonly component: AssistantMessageComponent | ToolExecutionComponent; key: string; readonly argsComplete?: boolean }

/** The same public message components as InteractiveMode, fed only bounded, safe display data. */
export class HelperChat {
  private request: string | undefined;
  private prompt: UserMessageComponent | undefined;
  private readonly cached = new Map<string, Cached>();
  private hits: { start: number; end: number; component: Component }[] = [];
  private styleKey = "";
  private transformers: readonly MarkdownTransformer[] = [];
  private expanded = false;
  constructor(private readonly tui: () => TUI | undefined, private readonly cwd: string,
    private readonly style?: () => HelperChatStyle) {}
  renderHelper(helper: ConversationSource, width: number): string[] {
    const style = this.style?.() ?? { outputPad: 1, markdownTheme: getMarkdownTheme(), markdownTransformers: [], renderers: () => undefined };
    const styleKey = `${style.outputPad}/${style.markdownTheme.codeBlockIndent ?? ""}`;
    if (this.request !== helper.request || this.styleKey !== styleKey || this.transformers.length !== style.markdownTransformers.length
      || style.markdownTransformers.some((transformer, index) => transformer !== this.transformers[index])) {
      this.clear(); this.request = helper.request; this.styleKey = styleKey; this.transformers = style.markdownTransformers;
    }
    this.prompt ??= new UserMessageComponent(helperText(helper.prompt), style.markdownTheme, style.outputPad, style.markdownTransformers);
    const rows: string[] = [];
    this.hits = [];
    const append = (component: Component) => {
      const lines = component.render(width);
      this.hits.push({ start: rows.length, end: rows.length + lines.length, component }); rows.push(...lines);
    };
    append(this.prompt);
    const retained = new Set<string>();
    const messages: readonly HelperMessage[] = helper.result !== undefined && !helper.messages.some(message => message.kind === "assistant" && (message.textBlocks?.join("") ?? message.text).trim() === helper.result)
      ? [...helper.messages, { id: "returned-result", kind: "assistant", text: helper.result, stopReason: "pending" }] : helper.messages;
    for (const message of messages) {
      retained.add(message.id);
      const key = JSON.stringify([message.text, message.textBlocks, message.arguments, message.phase, message.stopReason, message.errorMessage, message.hasToolCalls]);
      let cached = this.cached.get(message.id);
      if (message.kind === "assistant") {
        const component = cached?.component instanceof AssistantMessageComponent ? cached.component
          : new AssistantMessageComponent(undefined, true, style.markdownTheme, "", style.outputPad, style.markdownTransformers);
        if (!cached || cached.key !== key) component.updateContent({ content: (message.textBlocks ?? [message.text]).map(text => ({ type: "text", text: helperText(text) })),
          stopReason: message.stopReason ?? "pending", errorMessage: message.errorMessage === undefined ? undefined : helperText(message.errorMessage), hasToolCalls: message.hasToolCalls }, helper.phase === "running" && message.stopReason === undefined);
        cached = { component, key };
      } else {
        const tui = this.tui();
        if (!tui) continue; // A headless host has no interactive tool-renderer context.
        const args = toolArgs(message.arguments);
        const reused = cached?.component instanceof ToolExecutionComponent && cached.argsComplete === args.complete;
        const component = reused && cached?.component instanceof ToolExecutionComponent ? cached.component
          : new ToolExecutionComponent(helperText(message.tool ?? "unavailable"), message.id, args.value,
            { showImages: false, outputPad: style.outputPad }, args.complete ? style.renderers(message.tool ?? "") : undefined, tui, this.cwd);
        if (!reused) component.setExpanded(this.expanded);
        if (!cached || cached.key !== key || cached.argsComplete !== args.complete) {
          component.updateArgs(args.value);
          if (args.complete) component.setArgsComplete();
          if (message.phase !== "requested") component.markExecutionStarted();
          if (message.phase === "completed" || message.phase === "error") component.updateResult({
            content: [{ type: "text", text: helperText(message.text) }], isError: message.phase === "error",
          });
        }
        cached = { component, key, argsComplete: args.complete };
      }
      this.cached.set(message.id, cached); append(cached.component);
    }
    for (const id of this.cached.keys()) if (!retained.has(id)) this.cached.delete(id);
    return rows;
  }
  handleMouse(event: TuiMouseEvent): ReturnType<NonNullable<Component["handleMouse"]>> {
    const hit = this.hits.find(hit => event.y >= hit.start && event.y < hit.end);
    return hit?.component.handleMouse?.({ ...event, y: event.y - hit.start, height: hit.end - hit.start });
  }
  expandTools(expanded: boolean): void {
    this.expanded = expanded;
    for (const item of this.cached.values()) if (item.component instanceof ToolExecutionComponent) item.component.setExpanded(expanded);
  }
  invalidate(): void {
    this.prompt?.invalidate();
    for (const item of this.cached.values()) item.component.invalidate();
  }
  clear(): void { this.prompt = undefined; this.cached.clear(); this.hits = []; this.request = undefined; this.styleKey = ""; }
}
