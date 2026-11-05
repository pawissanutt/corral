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

/** Literal inline visibility only; no stylesheet fetching or computed CSS evaluation. */
function inlineHidden(style: string): boolean {
  const properties = new Map<string, { value: string; important: boolean }>();
  for (const declaration of style.replace(/\/\*[\s\S]*?\*\//g, "").toLowerCase().split(";")) {
    const colon = declaration.indexOf(":");
    if (colon < 0) continue;
    const name = declaration.slice(0, colon).trim();
    if (name !== "display" && name !== "visibility") continue;
    const raw = declaration.slice(colon + 1).trim();
    const important = /!\s*important\s*$/.test(raw);
    const value = raw.replace(/!\s*important\s*$/, "").trim();
    if (value && (!properties.get(name)?.important || important)) properties.set(name, { value, important });
  }
  return properties.get("display")?.value === "none" || properties.get("visibility")?.value === "hidden";
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
      "script,style,noscript,template,iframe,object,embed,meta,link,base,svg,canvas,nav,aside,footer,[hidden],[aria-hidden='true']",
    )) node.remove();
    for (const node of document.querySelectorAll("[style]")) {
      if (inlineHidden(node.getAttribute("style") ?? "")) node.remove();
    }
    for (const link of document.querySelectorAll("a[href]")) {
      const href = safeLink(link.getAttribute("href") ?? "", base);
      if (href) link.setAttribute("href", href); else link.removeAttribute("href");
    }
    for (const image of document.querySelectorAll("img")) {
      // Keep alt text, never inline data payloads or image downloads.
      image.replaceWith(document.createTextNode(image.getAttribute("alt") ?? ""));
    }
    // An empty or sanitizer-emptied landmark must not hide useful content elsewhere.
    let main: typeof document.body | undefined;
    for (const node of document.querySelectorAll("main,[role='main'],article")) {
      if (node.textContent?.trim()) { main = node; break; }
    }
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
    const body = converter.turndown(root).replace(/\n{3,}/g, "\n\n").trim();
    if (body || root === document.body) return { body, method };
    return { body: converter.turndown(document.body).replace(/\n{3,}/g, "\n\n").trim(), method: "body" };
  } catch (cause) {
    throw new WebError("web_conversion_failed", "HTML could not be converted locally; use format raw for bounded source", { cause });
  }
}
