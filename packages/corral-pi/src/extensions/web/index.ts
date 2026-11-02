import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import type { WebAccess } from "./client.ts";

export const WEB_TOOL_NAMES = ["web_fetch", "web_search"] as const;

export type SubscriptionSearch = (id: string, query: string, count: number, signal?: AbortSignal) => Promise<string>;

export function webTools(access: WebAccess, subscription?: SubscriptionSearch): readonly ToolDefinition[] {
  return [
    {
      name: "web_fetch",
      label: "Fetch public web page",
      description: "Read a public HTTP(S) URL with GET and no cookies or credentials. Output is byte- and time-bounded; "
        + "private, loopback, link-local and metadata targets are refused, including redirects. Returns raw text, status "
        + "and content type. An optional configured host list can narrow public access.",
      parameters: Type.Object({ url: Type.String({ description: "Public HTTP(S) URL" }) }),
      async execute(_id, params: { url: string }, signal) {
        return { content: [{ type: "text", text: JSON.stringify(await access.fetch(params.url, signal)) }], details: undefined };
      },
    },
    {
      name: "web_search",
      label: "Search web",
      description: subscription
        ? "Search using the current Codex or Claude subscription. Returns a sourced answer and URLs; uses subscription quota. No paid-API fallback."
        : "Search the configured SearXNG service. Returns titles, URLs and snippets, not page bodies. "
          + "Without a configured endpoint, returns setup instructions; public web_fetch remains available.",
      parameters: Type.Object({
        query: Type.String({ minLength: 1, maxLength: 4096 }),
        num_results: Type.Optional(Type.Integer({ minimum: 1, maximum: 20 })),
      }),
      async execute(id, params: { query: string; num_results?: number }, signal) {
        return {
          content: [{ type: "text", text: subscription
            ? await subscription(id, params.query, params.num_results ?? 5, signal)
            : JSON.stringify(await access.search(params.query, params.num_results ?? 5, signal)) }],
          details: undefined,
        };
      },
    },
  ];
}
