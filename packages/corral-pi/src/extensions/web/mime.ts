import { WebError } from "./policy.ts";

export function mediaType(value: string): string {
  return value.split(";", 1)[0]?.trim().toLowerCase() ?? "";
}

/** Empty/generic MIME needs a bounded body sample; known binary formats never become text. */
export function requireTextType(value: string): void {
  const mime = mediaType(value);
  if (!mime || mime === "application/octet-stream" || mime.startsWith("text/") ||
      ["application/json", "application/xml", "application/xhtml+xml", "application/javascript"].includes(mime) ||
      mime.endsWith("+json") || mime.endsWith("+xml")) return;
  throw new WebError("web_unsupported_content", "web fetch supports textual content only; binary conversion is unavailable");
}

export function requireTextSample(body: string, contentType: string): void {
  const sample = body.slice(0, 4096);
  const mime = mediaType(contentType);
  const pattern = !mime || mime === "application/octet-stream"
    ? /[\u0000-\u0008\u000b\u000c\u000e-\u001f\ufffd]/g : /[\u0000-\u0008\u000b\u000c\u000e-\u001f]/g;
  const controls = sample.match(pattern)?.length ?? 0;
  if (sample.includes("\0") || (controls >= 3 && controls / sample.length > 0.01)) {
    throw new WebError("web_unsupported_content", "response sample is binary or cannot be decoded as UTF-8 text");
  }
}
