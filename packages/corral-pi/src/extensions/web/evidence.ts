// Evidence comes from provider events, never a model's assertion that it searched.
export function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}
export interface SearchSource { readonly title: string; readonly url: string }

export class SearchEvidence {
  private readonly sources = new Map<string, SearchSource>();
  private readonly claudeCalls = new Set<string>();
  private verified = false;
  private rejected = false;
  private messagesStreamPending = false;
  private readonly pendingResults = new Map<number, Record<string, unknown>>();
  get searched(): boolean { return this.verified && !this.rejected && !this.messagesStreamPending; }

  private source(value: unknown): void {
    const entry = record(value);
    if (typeof entry.url !== "string" || entry.url.length > 4096 || /[\s\x00-\x1f\x7f]/.test(entry.url)) return;
    const url = URL.parse(entry.url);
    if (!url || !["http:", "https:"].includes(url.protocol) || url.username || url.password) return;
    if (this.sources.size >= 100 || this.sources.has(url.href)) return;
    this.sources.set(url.href, { url: url.href, title: typeof entry.title === "string" ? entry.title.slice(0, 512) : url.href });
  }
  private claudeSources(value: unknown, depth = 0): void {
    if (depth > 10) return;
    if (Array.isArray(value)) {
      for (const item of value) this.claudeSources(item, depth + 1);
    } else if (typeof value === "string") {
      // Claude Code's WebSearch text result carries a JSON Links array.
      const links = /Links:\s*(\[[^\n]*\])/.exec(value)?.[1];
      if (links) {
        try { this.claudeSources(JSON.parse(links), depth + 1); } catch (error) {
          if (!(error instanceof SyntaxError)) throw error;
        }
      }
      for (const match of value.matchAll(/\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g)) {
        this.source({ title: match[1], url: match[2] });
      }
    } else {
      const entry = record(value);
      this.source(entry);
      for (const child of Object.values(entry)) this.claudeSources(child, depth + 1);
    }
  }
  private codexItem(value: unknown): void {
    const item = record(value);
    if (item.type === "web_search_call" && ["failed", "incomplete"].includes(String(item.status))) this.rejected = true;
    if (item.type === "web_search_call" && item.status === "completed" && typeof item.id === "string" && item.id &&
      record(item.action).type === "search") {
      this.verified = true;
      const sources = record(item.action).sources;
      if (Array.isArray(sources)) for (const source of sources) this.source(source);
    }
    if (item.type === "message" && Array.isArray(item.content)) {
      for (const part of item.content) {
        if (record(part).type === "refusal") this.rejected = true;
        const annotations = record(part).annotations;
        if (Array.isArray(annotations)) for (const annotation of annotations) {
          if (record(annotation).type === "url_citation") this.source(annotation);
        }
      }
    }
  }
  observe(provider: string, value: unknown): void {
    const event = record(value);
    if (provider === "openai-codex" || provider === "openai") {
      if (["response.refusal.delta", "response.refusal.done", "response.failed", "response.incomplete"].includes(String(event.type))) {
        this.rejected = true;
      }
      if (event.type === "response.output_item.done") this.codexItem(event.item);
      if (event.type === "response.completed" || event.type === "response.done") {
        const output = record(event.response).output;
        if (Array.isArray(output)) for (const item of output) this.codexItem(item);
      }
    } else if (provider === "anthropic-api") {
      if (event.type === "message_start") this.messagesStreamPending = true;
      if (event.type === "message_stop") this.messagesStreamPending = false;
      if (event.type === "message_delta" && ["refusal", "pause_turn"].includes(String(record(event.delta).stop_reason))) {
        this.rejected = true;
      }
      if (event.type === "content_block_start") {
        const block = record(event.content_block);
        if (block.type === "server_tool_use" && block.name === "web_search" && typeof block.id === "string" && block.id) {
          this.claudeCalls.add(block.id);
        } else if (block.type === "web_search_tool_result" && typeof event.index === "number") {
          this.pendingResults.set(event.index, block);
        }
      } else if (event.type === "content_block_stop" && typeof event.index === "number") {
        const block = this.pendingResults.get(event.index);
        this.pendingResults.delete(event.index);
        if (block && typeof block.tool_use_id === "string" && this.claudeCalls.delete(block.tool_use_id)) {
          if (!Array.isArray(block.content) || block.content.some((result) => record(result).type !== "web_search_result")) {
            this.rejected = true;
          } else {
            this.verified = true;
            for (const result of block.content) this.source(result);
          }
        }
      } else if (event.type === "content_block_delta") {
        const delta = record(event.delta);
        const citation = record(delta.citation);
        if (delta.type === "citations_delta" && citation.type === "web_search_result_location") this.source(citation);
      }
    } else if (provider === "anthropic") {
      const content = record(event.message).content;
      if (!Array.isArray(content)) return;
      for (const value of content) {
        const block = record(value);
        if (event.type === "assistant" && block.type === "tool_use" && block.name === "WebSearch" && typeof block.id === "string") {
          this.claudeCalls.add(block.id);
        } else if (event.type === "user" && block.type === "tool_result" && typeof block.tool_use_id === "string" &&
            this.claudeCalls.delete(block.tool_use_id)) {
          if (block.is_error === true) { this.rejected = true; continue; }
          this.verified = true;
          this.claudeSources(block.content);
          this.claudeSources(event.tool_use_result);
        }
      }
    }
  }
  results(): SearchSource[] { return this.searched ? [...this.sources.values()] : []; }
}
