// LSP base-protocol framing: `Content-Length: <n>\r\n\r\n<n bytes of UTF-8 JSON>`. The decoder
// is bounded: a header block over 8 KiB or a body over the frame limit is a protocol error,
// so a runaway server cannot grow the host's memory without limit.

const HEADER_END = Buffer.from("\r\n\r\n");
const MAX_HEADER_BYTES = 8 * 1024;

export class LspFrameError extends Error {
  readonly code = "lsp_protocol_error";
  constructor(reason: string) {
    super(`lsp_protocol_error: ${reason}`);
  }
}

export function encodeFrame(message: unknown): Buffer {
  const body = Buffer.from(JSON.stringify(message), "utf8");
  return Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, "ascii"), body]);
}

/** Incremental decoder; `push` returns every complete message the chunk finished. */
export class FrameDecoder {
  private buffer: Buffer = Buffer.alloc(0);

  constructor(private readonly maxFrameBytes: number) {}

  push(chunk: Buffer): unknown[] {
    this.buffer = this.buffer.length === 0 ? chunk : Buffer.concat([this.buffer, chunk]);
    const messages: unknown[] = [];
    for (;;) {
      const end = this.buffer.indexOf(HEADER_END);
      if (end < 0) {
        if (this.buffer.length > MAX_HEADER_BYTES) throw new LspFrameError("header block exceeds 8 KiB");
        return messages;
      }
      if (end > MAX_HEADER_BYTES) throw new LspFrameError("header block exceeds 8 KiB");
      const length = contentLength(this.buffer.subarray(0, end).toString("ascii"));
      if (length > this.maxFrameBytes) {
        throw new LspFrameError(`frame of ${length} bytes exceeds the ${this.maxFrameBytes}-byte limit`);
      }
      const start = end + HEADER_END.length;
      if (this.buffer.length < start + length) return messages;
      const body = this.buffer.subarray(start, start + length).toString("utf8");
      this.buffer = this.buffer.subarray(start + length);
      try {
        messages.push(JSON.parse(body));
      } catch {
        throw new LspFrameError("frame body is not JSON");
      }
    }
  }
}

function contentLength(headers: string): number {
  for (const line of headers.split("\r\n")) {
    const match = /^content-length:\s*(\d+)\s*$/i.exec(line);
    if (match?.[1]) return Number(match[1]);
  }
  throw new LspFrameError("frame has no Content-Length header");
}
