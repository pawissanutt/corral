import { Readability } from "@mozilla/readability";
import { parseHTML } from "linkedom";
import TurndownService from "turndown";

import type { WebPage } from "./client.ts";
import { mediaType, requireTextSample, requireTextType } from "./mime.ts";
import { WebError } from "./policy.ts";

export type ContentFormat = "markdown" | "raw";
export type ContentMethod = "article" | "main" | "body" | "text" | "raw";
export interface RenderedContent { readonly body: string; readonly method: ContentMethod }

function safeLink(value: string, base: string): string | undefined {
  const url = URL.parse(value, base);
  return url && ["http:", "https:", "mailto:"].includes(url.protocol) && !url.username && !url.password
    ? url.href : undefined;
}

/** Inert DOM only: no globals, scripts, subresource loading, browser or remote reader. */
export function renderContent(page: WebPage, format: ContentFormat): RenderedContent {
  requireTextType(page.content_type);
  requireTextSample(page.body, page.content_type);
  if (format === "raw") return { body: page.body, method: "raw" };
  const mime = mediaType(page.content_type);
  const html = mime.includes("html") || ((!mime || mime === "application/octet-stream" || mime === "text/plain") &&
    /^\s*(?:<!doctype\s+html\b|<html\b|<head\b|<body\b|<article\b|<main\b|<(?:div|section|p|h[1-6]|table|script|style|nav)\b)/i.test(page.body.slice(0, 512)));
  if (!html) return { body: page.body, method: "text" };
  try {
    const document = parseHTML(`<html><head></head><body></body></html>`).document;
    // Parsing as a fragment also handles omitted html/body containers without losing their text.
    const parsed = parseHTML(page.body).document;
    document.body.innerHTML = parsed.documentElement?.localName === "html" ? parsed.documentElement.innerHTML : page.body;
    if (document.querySelectorAll("*").length > 20000) {
      throw new WebError("web_conversion_failed", "HTML exceeds the local conversion element limit");
    }
    const base = URL.parse(document.querySelector("base[href]")?.getAttribute("href") ?? "", page.url)?.href ?? page.url;
    for (const node of document.querySelectorAll(
      "script,style,noscript,template,iframe,object,embed,meta,link,base,svg,canvas,[hidden],[aria-hidden='true']",
    )) node.remove();
    for (const link of document.querySelectorAll("a[href]")) {
      const href = safeLink(link.getAttribute("href") ?? "", base);
      if (href) link.setAttribute("href", href); else link.removeAttribute("href");
    }
    for (const image of document.querySelectorAll("img")) {
      // Keep alt text, never inline data payloads or image downloads.
      image.replaceWith(document.createTextNode(image.getAttribute("alt") ?? ""));
    }
    const main = document.querySelector("main,[role='main'],article");
    let root = main ?? document.body;
    let method: ContentMethod = main ? "main" : "body";
    if (!main) {
      const copy = parseHTML(document.toString()).document;
      Object.defineProperty(copy, "documentURI", { value: page.url });
      const article = new Readability(copy, { charThreshold: 80, maxElemsToParse: 20000, disableJSONLD: true }).parse();
      if (article?.content && article.textContent?.trim()) {
        const container = document.createElement("div");
        container.innerHTML = article.content;
        root = container;
        method = "article";
      }
    }
    for (const noise of root.querySelectorAll("nav,aside,footer")) noise.remove();
    const converter = new TurndownService({ headingStyle: "atx", codeBlockStyle: "fenced", bulletListMarker: "-" });
    // Keep ordinary documentation tables structured without depending on browser-only table.rows APIs.
    converter.addRule("cells", { filter: ["th", "td"], replacement: (text) => ` ${text.trim().replace(/\|/g, "\\|").replace(/\n/g, " ")} |` });
    converter.addRule("rows", { filter: "tr", replacement: (text, node) => {
      const count = node.querySelectorAll("th,td").length;
      const separator = count > 0 && node.querySelectorAll("th").length === count
        ? `\n|${Array.from({ length: count }, () => " --- |").join("")}` : "";
      return `\n|${text}${separator}`;
    } });
    converter.addRule("tables", { filter: "table", replacement: (text) => `\n\n${text.trim()}\n\n` });
    return { body: converter.turndown(root).replace(/\n{3,}/g, "\n\n").trim(), method };
  } catch (cause) {
    throw new WebError("web_conversion_failed", "HTML could not be converted locally; use format raw for bounded source", { cause });
  }
}
