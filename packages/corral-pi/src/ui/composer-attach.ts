// What a paste into the composer becomes. An image (Pi's clipboard paste writes it to a temp file and inserts the
// path) or a dropped image file (a terminal pastes its path) is staged as an ImageContent and leaves `[Image #n]` at
// the cursor; the model receives the image itself when the message is sent, the markers renumbered in reading order.
// Long clipboard text goes through pi-tui's own bracketed-paste fold (`[paste #n …]`), as a terminal paste does.
// Path detection is ported from oh-my-pi's prompt/custom-editor.ts (same owner).
import { readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { basename } from "node:path";
import { fileURLToPath } from "node:url";
import type { ImageContent } from "@earendil-works/pi-ai";
import { matchesKey, type KeyId } from "@earendil-works/pi-tui";
import { sniffImageType } from "../tools/image-type.ts";
import { PASTE_START } from "./build/choice-dock.ts";

export { PASTE_START };
export const PASTE_END = "\x1b[201~";
/** `[Image #n]`: the nth image staged for the message being written. */
export const IMAGE_MARKER = /\[Image #([1-9]\d*)\]/g;
export const MAX_IMAGES = 8;
export const MAX_IMAGE_BYTES = 20 * 1024 * 1024;
/** pi-tui Editor.handlePaste's own fold thresholds. */
export function isLargePaste(text: string): boolean { return text.split("\n").length > 10 || text.length > 1000; }

const IMAGE_PATH = /\.(?:png|jpe?g|gif|webp)$/i;
const ESCAPED = /\\([\\\s'"()[\]{}&;<>|?*!$`])/g;
const PATH_START = String.raw`(?:\/|~\/|file:\/\/|[A-Za-z]:[\\/])`;
const ANCHORED = new RegExp(`^${PATH_START}`);
/** A second path after unescaped whitespace: two dropped files, not one path with a space in its name. */
const SECOND_PATH = new RegExp(String.raw`(?<!\\)\s${PATH_START}`);
function segments(text: string): string[] | undefined {
  const out: string[] = [];
  let segment = "", quote: string | undefined, escaped = false;
  for (const char of text) {
    if (escaped) { segment += char; escaped = false; continue; }
    if (char === "\\") { segment += char; escaped = true; continue; }
    if (quote) { segment += char; if (char === quote) quote = undefined; continue; }
    if (char === "'" || char === '"') { segment += char; quote = char; continue; }
    if (char === " " || char === "\t" || char === "\r" || char === "\n") { if (segment) out.push(segment); segment = ""; continue; }
    segment += char;
  }
  if (escaped || quote) return undefined;
  if (segment) out.push(segment);
  return out.length ? out : undefined;
}

function normalise(segment: string, env: NodeJS.ProcessEnv): string | undefined {
  const trimmed = segment.trim(), first = trimmed[0];
  let path = trimmed.length > 1 && (first === "'" || first === '"') && trimmed.endsWith(first) ? trimmed.slice(1, -1) : trimmed;
  if (/^file:\/\//i.test(path)) {
    try { path = fileURLToPath(path); } catch {
      try { path = fileURLToPath(path, { windows: false }); } catch { return undefined; }
    }
  } else {
    path = path.replace(ESCAPED, "$1");
  }
  if (path.startsWith("~/")) path = homedir() + path.slice(1);
  const drive = /^([A-Za-z]):[\\/](.*)$/.exec(path);
  if (drive && (env.WSL_DISTRO_NAME || env.WSL_INTEROP)) path = `/mnt/${drive[1]!.toLowerCase()}/${drive[2]!.replaceAll("\\", "/")}`;
  return path;
}

const isImagePath = (path: string | undefined): path is string => path !== undefined && path.startsWith("/") && IMAGE_PATH.test(path);

/**
 * The image files `text` names when it is nothing but one or more explicit image paths (quoted, shell-escaped,
 * `file://`, `~/`, or a Windows drive path under WSL), or undefined for any other text.
 */
export function pastedImagePaths(text: string, env: NodeJS.ProcessEnv = process.env): string[] | undefined {
  const trimmed = text.trim();
  if (!trimmed) return undefined;
  const paths = segments(trimmed)?.map(segment => normalise(segment, env));
  if (paths?.every(isImagePath)) return paths;
  // One path whose name has unescaped spaces (a screenshot's name), never two paths or a sentence.
  if (/[\r\n]/.test(trimmed) || !ANCHORED.test(trimmed) || SECOND_PATH.test(trimmed)) return undefined;
  const whole = normalise(trimmed, env);
  return isImagePath(whole) ? [whole] : undefined;
}

/** The image at `path` as an ImageContent, or why it cannot be one. */
export function readImageFile(path: string): { image: ImageContent; name: string } | { error: string } {
  const name = basename(path);
  let stat;
  try { stat = statSync(path); } catch { return { error: `${name} was not found` }; }
  if (!stat.isFile()) return { error: `${name} is not a file` };
  if (stat.size > MAX_IMAGE_BYTES) return { error: `${name} is larger than 20 MB` };
  let bytes: Buffer;
  try { bytes = readFileSync(path); } catch { return { error: `${name} could not be read` }; }
  const mimeType = sniffImageType(bytes);
  if (!mimeType) return { error: `${name} is not a PNG, JPEG, GIF or WebP image` };
  return { image: { type: "image", data: bytes.toString("base64"), mimeType }, name };
}

/** The images staged for the message being written, by the number of their `[Image #n]` marker. */
export class StagedImages {
  private readonly staged = new Map<number, { readonly image: ImageContent; readonly name: string }>();
  private next = 0;
  get count(): number { return this.staged.size; }
  /** How many staged images `draft` marks: the images the message being written would carry. */
  markedIn(draft: string): number {
    return new Set([...draft.matchAll(IMAGE_MARKER)].map(match => Number(match[1])).filter(n => this.staged.has(n))).size;
  }
  /** Stages one image and returns its marker; the caller checks `count` against MAX_IMAGES first. */
  stage(image: ImageContent, name: string): string {
    this.next += 1;
    this.staged.set(this.next, { image, name });
    return `[Image #${this.next}]`;
  }
  /**
   * The images `text` still marks, in reading order, with its markers renumbered to match; an image whose marker was
   * deleted is dropped. Everything staged clears.
   */
  take(text: string, draft = ""): { text: string; images: ImageContent[]; dropped: number } {
    const marked = (source: string) => [...source.matchAll(IMAGE_MARKER)].map(match => Number(match[1])).filter(n => this.staged.has(n));
    const order = [...new Set(marked(text))];
    const renamed = text.replace(IMAGE_MARKER, (marker, n: string) => {
      const at = order.indexOf(Number(n));
      return at < 0 ? marker : `[Image #${at + 1}]`;
    });
    const images = order.map(n => this.staged.get(n)!.image);
    // An image the draft still being written marks stays staged for it (Pi can send an earlier held message first).
    const kept = new Set(marked(draft).filter(n => !order.includes(n)));
    const dropped = this.count - order.length - kept.size;
    for (const n of [...this.staged.keys()]) if (!kept.has(n)) this.staged.delete(n);
    if (this.staged.size === 0) this.next = 0;
    return { text: renamed, images, dropped };
  }
  clear(): void { this.staged.clear(); this.next = 0; }
  /**
   * Stages again the images of a queued message pulled back into the draft. Its text marks them `[Image #1]` to
   * `[Image #k]` in the order they were sent, so each marker is renumbered to its image's new staging number.
   */
  restage(text: string, images: readonly ImageContent[]): string {
    const markers = new Map<number, string>();
    return text.replace(IMAGE_MARKER, (marker, n: string) => {
      const at = Number(n), image = images[at - 1];
      if (!image) return marker;
      if (!markers.has(at)) markers.set(at, this.stage(image, `image ${at}`));
      return markers.get(at)!;
    });
  }
}

/** The stock editor's own operations, which ComposerPaste calls instead of the composer's overrides. */
export interface PasteBase {
  /** The stock editor's handleInput (super.handleInput). */
  input(data: string): void;
  /** The stock editor's insertTextAtCursor (super.insertTextAtCursor). */
  insert(text: string): void;
  /** Pi's clipboard paste: editor.onPasteImage?.(). */
  clipboard(): void;
  text(): string;
  cursor(): { line: number; col: number };
  lines(): string[];
}
export interface ComposerPasteDeps {
  readonly images: StagedImages;
  imageInput(): { readonly accepts: boolean; readonly model: string };
  notice(text: string, tone: "warning" | "error"): void;
  /** Keys bound to Pi's app.clipboard.pasteImage. */
  pasteKeys(): readonly string[];
  now?(): number;
}

/** A clipboard paste's text arrives within this long of its key, or it is ordinary inserted text. */
const CLIPBOARD_WINDOW_MS = 10_000;

/** Routes the composer's pastes: image paths become staged images, long clipboard text folds. */
export class ComposerPaste {
  private buffer: string | undefined;
  private clipboardAt: number | undefined;
  constructor(private readonly deps: ComposerPasteDeps) {}
  private now(): number { return this.deps.now?.() ?? Date.now(); }

  /** Takes `data` before the stock editor sees it; true when it was consumed here. */
  handle(data: string, base: PasteBase): boolean {
    if (this.buffer !== undefined || data.includes(PASTE_START)) {
      let chunk = data;
      if (this.buffer === undefined) {
        const start = data.indexOf(PASTE_START);
        if (start > 0) base.input(data.slice(0, start));
        chunk = data.slice(start + PASTE_START.length);
        this.buffer = "";
      }
      this.buffer += chunk;
      const end = this.buffer.indexOf(PASTE_END);
      if (end < 0) return true;
      const payload = this.buffer.slice(0, end), rest = this.buffer.slice(end + PASTE_END.length);
      this.buffer = undefined;
      this.pasted(payload, base);
      if (rest && !this.handle(rest, base)) base.input(rest);
      return true;
    }
    if (this.deps.pasteKeys().some(key => matchesKey(data, key as KeyId))) this.clipboardAt = this.now();
    return false;
  }

  /** Text Pi's clipboard paste inserts; true when it was taken here instead. */
  inserted(text: string, base: PasteBase): boolean {
    const at = this.clipboardAt;
    this.clipboardAt = undefined;
    if (at === undefined || this.now() - at > CLIPBOARD_WINDOW_MS) return false;
    if (this.attachPaths(text, base)) return true;
    if (!isLargePaste(text)) return false;
    base.input(PASTE_START + text + PASTE_END);
    return true;
  }

  private pasted(payload: string, base: PasteBase): void {
    // A terminal that keeps Ctrl+V for itself pastes nothing when the clipboard holds no text: read it as Pi does.
    if (payload === "") { this.clipboardAt = this.now(); base.clipboard(); return; }
    if (this.attachPaths(payload, base)) return;
    base.input(PASTE_START + payload + PASTE_END);
  }

  // Pi's clipboard temp file stays where Pi wrote it, as in stock Pi: a path alone cannot prove which file Pi made.
  private attachPaths(text: string, base: PasteBase): boolean {
    // A bash command keeps the paths it is given.
    if (base.text().trimStart().startsWith("!")) return false;
    const paths = pastedImagePaths(text);
    if (!paths) return false;
    const input = this.deps.imageInput();
    if (!input.accepts) { this.deps.notice(`${input.model} does not take images; the file path was inserted instead`, "warning"); return false; }
    // An image whose marker was deleted or cleared no longer counts; one Pi has yet to send left the draft already.
    if (this.deps.images.markedIn(base.text()) + paths.length > MAX_IMAGES) {
      this.deps.notice(`At most ${MAX_IMAGES} images per message; the path was inserted instead`, "warning");
      return false;
    }
    const loaded: { image: ImageContent; name: string }[] = [];
    for (const path of paths) {
      const read = readImageFile(path);
      if ("error" in read) { this.deps.notice(`Not attached: ${read.error}; the path was inserted instead`, "warning"); return false; }
      loaded.push(read);
    }
    const markers = loaded.map(({ image, name }) => this.deps.images.stage(image, name)).join(" ");
    const { line, col } = base.cursor(), row = base.lines()[line] ?? "";
    const before = row.slice(0, col).at(-1), after = row[col];
    const lead = before !== undefined && /\S/.test(before) ? " " : "", trail = after !== undefined && /\S/.test(after) ? " " : "";
    base.insert(lead + markers + trail);
    return true;
  }
}
