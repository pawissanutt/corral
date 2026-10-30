import { z } from "zod";

import type { Fetch } from "../../auth/types.ts";
import { checkPublicTarget, type Lookup, resolveHost, WebError } from "./policy.ts";
import type { WebSettings } from "./settings.ts";

export interface WebPage {
  readonly url: string;
  readonly status: number;
  readonly content_type: string;
  readonly body: string;
  readonly bytes: number;
  readonly truncated: boolean;
}

const SearchResponse = z.object({ results: z.array(z.object({
  title: z.string(), url: z.string(), content: z.string().optional(),
})) });
type SearchItem = z.infer<typeof SearchResponse>["results"][number];
export type SearchResult =
  | { readonly configured: false; readonly setup: string; readonly results: readonly SearchItem[] }
  | { readonly configured: true; readonly results: readonly SearchItem[]; readonly truncated: boolean };

export interface WebAccess {
  fetch(url: string, signal?: AbortSignal): Promise<WebPage>;
  search(query: string, count: number, signal?: AbortSignal): Promise<SearchResult>;
}

export interface WebAccessOptions {
  readonly lookup?: Lookup;
  readonly allowed?: () => void;
  readonly refused?: (destination: string) => void;
}

/** Abort ordinary DNS and stream waits too, not just fetch's connection phase. */
async function abortable<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  signal.throwIfAborted();
  let listener: (() => void) | undefined;
  const cancelled = new Promise<never>((_resolve, reject) => {
    listener = () => reject(signal.reason);
    signal.addEventListener("abort", listener, { once: true });
  });
  try {
    return await Promise.race([promise, cancelled]);
  } finally {
    if (listener) signal.removeEventListener("abort", listener);
  }
}

async function readBody(response: Response, max: number, signal: AbortSignal) {
  const reader = response.body?.getReader();
  if (!reader) return { body: "", bytes: 0, truncated: false };
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  let truncated = false;
  try {
    for (;;) {
      const next = await abortable(reader.read(), signal);
      if (next.done) break;
      const keep = Math.min(next.value.byteLength, max - bytes);
      chunks.push(next.value.slice(0, keep));
      bytes += keep;
      if (keep < next.value.byteLength) { truncated = true; break; }
    }
    return { body: new TextDecoder().decode(Buffer.concat(chunks), { stream: truncated }), bytes, truncated };
  } finally {
    await reader.cancel();
    reader.releaseLock();
  }
}

function target(value: string): URL {
  const url = URL.parse(value);
  if (!url || !["http:", "https:"].includes(url.protocol) || url.username || url.password) {
    throw new WebError("web_target_refused", "use an HTTP(S) URL without credentials");
  }
  url.hash = "";
  return url;
}

/**
 * Purpose-scoped transports. Neither path receives provider auth or caller-selected headers.
 * Search alone may reach the configured local service; public fetch cannot borrow that exception.
 */
export function createWebAccess(settings: WebSettings, send: Fetch, options: WebAccessOptions = {}): WebAccess {
  const deadline = (signal?: AbortSignal) => signal
    ? AbortSignal.any([signal, AbortSignal.timeout(settings.timeout_ms)])
    : AbortSignal.timeout(settings.timeout_ms);
  const request = async (url: URL, signal: AbortSignal, accept: string) => {
    options.allowed?.();
    return abortable(send(url.href, {
      method: "GET", headers: { accept }, redirect: "manual", credentials: "omit", referrerPolicy: "no-referrer", signal,
    }), signal);
  };
  return {
    async fetch(value, suppliedSignal) {
      const signal = deadline(suppliedSignal);
      let url = target(value);
      for (let redirects = 0; ; redirects++) {
        try {
          await abortable(checkPublicTarget(url, settings.hosts, options.lookup ?? resolveHost), signal);
        } catch (error) {
          if (error instanceof WebError) options.refused?.(`${url.origin}${url.pathname}`);
          throw error;
        }
        const response = await request(url, signal, "text/*, application/json");
        const location = response.headers.get("location");
        if ([301, 302, 303, 307, 308].includes(response.status) && location !== null) {
          await response.body?.cancel();
          if (redirects >= settings.max_redirects) throw new WebError("web_redirect_limit", "too many redirects");
          const next = URL.parse(location, url.href);
          if (!next) throw new WebError("web_target_refused", "invalid redirect URL");
          url = next;
          continue;
        }
        return {
          url: url.href, status: response.status, content_type: response.headers.get("content-type") ?? "",
          ...await readBody(response, settings.max_bytes, signal),
        };
      }
    },
    async search(query, count, suppliedSignal) {
      if (!settings.searxng_url) return {
        configured: false, setup: "Set agent.capabilities.<role>.web.searxng_url to your SearXNG base URL to enable search.",
        results: [],
      };
      const url = target(settings.searxng_url);
      if (!url.pathname.endsWith("/search")) url.pathname = `${url.pathname.replace(/\/$/, "")}/search`;
      url.search = "";
      url.searchParams.set("q", query);
      url.searchParams.set("format", "json");
      const signal = deadline(suppliedSignal);
      const response = await request(url, signal, "application/json");
      if (!response.ok) {
        await response.body?.cancel();
        throw new WebError("web_search_failed", `SearXNG returned HTTP ${response.status}; redirects are not followed`);
      }
      const result = await readBody(response, settings.max_bytes, signal);
      if (result.truncated) throw new WebError("web_response_too_large", "search response exceeded web.max_bytes");
      let json: unknown;
      try { json = JSON.parse(result.body); } catch (error) {
        if (!(error instanceof SyntaxError)) throw error;
        throw new WebError("web_search_failed", "SearXNG did not return JSON; enable its JSON output format");
      }
      const parsed = SearchResponse.safeParse(json);
      if (!parsed.success) throw new WebError("web_search_failed", "SearXNG returned an invalid results array");
      return { configured: true, results: parsed.data.results.slice(0, count), truncated: parsed.data.results.length > count };
    },
  };
}
