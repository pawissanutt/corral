// The message contract between a CodeKernel (host thread, kernel.ts) and its kernel worker
// (kernel-worker.ts), plus the output budget both sides enforce. Only plain structured-clone
// data crosses the thread boundary: tool inputs and results travel as JSON text.

export interface OutputLimits {
  /** UTF-8 bytes of printed or displayed text per cell, across all text items. */
  readonly maxTextBytes: number;
  /** UTF-8 bytes of the rendered return value. */
  readonly maxValueBytes: number;
  readonly maxImages: number;
  /** Decoded bytes per image. */
  readonly maxImageBytes: number;
  /** UTF-8 bytes of an error message; error names and codes are capped at ERROR_FIELD_BYTES. */
  readonly maxErrorBytes: number;
}

export const ERROR_FIELD_BYTES = 128;

export const IMAGE_MIME_TYPES: ReadonlySet<string> = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);

/** Model-visible output: only what a cell printed, displayed or returned. */
export type KernelOutput =
  | { readonly type: "text"; readonly text: string }
  | { readonly type: "image"; readonly data: string; readonly mimeType: string };

export interface WireError {
  readonly name: string;
  readonly message: string;
  readonly code?: string;
}

export type HostMessage =
  | { readonly op: "run"; readonly cell: string; readonly code: string; readonly limits: OutputLimits }
  | { readonly op: "result"; readonly cell: string; readonly call: string; readonly value?: string }
  | { readonly op: "failure"; readonly cell: string; readonly call: string; readonly error: WireError };

export type WorkerStatus = "ok" | "error";

export type WorkerMessage =
  | { readonly op: "ready"; readonly module: string }
  | { readonly op: "call"; readonly cell: string; readonly call: string; readonly tool: string; readonly input?: string }
  | { readonly op: "output"; readonly cell: string; readonly item: KernelOutput }
  | {
    readonly op: "done"; readonly cell: string; readonly status: WorkerStatus;
    readonly value?: string; readonly error?: WireError; readonly truncated: boolean;
  };

export const BASE64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;

export function base64Bytes(data: string): number {
  const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
  return (data.length / 4) * 3 - padding;
}

export function clipUtf8(text: string, bytes: number): string {
  const encoded = Buffer.from(text, "utf8");
  if (encoded.length <= bytes) return text;
  let end = Math.max(0, bytes);
  while (end > 0 && ((encoded[end] ?? 0) & 0xc0) === 0x80) end--;
  return encoded.subarray(0, end).toString("utf8");
}

/** One cell's output accounting. Anything past a limit is dropped and marks the cell truncated. */
export class OutputBudget {
  private textBytes = 0;
  private images = 0;
  truncated = false;

  constructor(private readonly limits: OutputLimits) {}

  text(text: string): string | undefined {
    const room = this.limits.maxTextBytes - this.textBytes;
    const size = Buffer.byteLength(text);
    if (size <= room) {
      this.textBytes += size;
      return text;
    }
    this.truncated = true;
    if (room <= 0) return undefined;
    this.textBytes = this.limits.maxTextBytes;
    return clipUtf8(text, room);
  }

  image(bytes: number, mimeType: string): boolean {
    if (this.images >= this.limits.maxImages || bytes > this.limits.maxImageBytes || !IMAGE_MIME_TYPES.has(mimeType)) {
      this.truncated = true;
      return false;
    }
    this.images++;
    return true;
  }

  error(error: WireError): WireError {
    const name = clipUtf8(error.name, ERROR_FIELD_BYTES);
    const message = clipUtf8(error.message, this.limits.maxErrorBytes);
    const code = error.code === undefined ? undefined : clipUtf8(error.code, ERROR_FIELD_BYTES);
    if (name !== error.name || message !== error.message || code !== error.code) this.truncated = true;
    return { name, message, ...(code === undefined ? {} : { code }) };
  }

  value(text: string | undefined): string | undefined {
    if (text === undefined || Buffer.byteLength(text) <= this.limits.maxValueBytes) return text;
    this.truncated = true;
    return clipUtf8(text, this.limits.maxValueBytes);
  }

  /** The admitted form of an output item, re-checked on the receiving side of the boundary. */
  admit(item: KernelOutput): KernelOutput | undefined {
    if (item.type === "image") {
      return BASE64.test(item.data) && this.image(base64Bytes(item.data), item.mimeType) ? item : undefined;
    }
    const text = this.text(item.text);
    return text === undefined ? undefined : { type: "text", text };
  }
}
