// LF-delimited frames assembled from raw socket bytes. A multibyte UTF-8 character may span
// two chunks, so bytes are kept until a whole line arrives and only then decoded, strictly.

export class FrameTooLarge extends Error {
  readonly code = "frame_too_large";
}

export class FrameInvalid extends Error {
  readonly code = "bad_frame";
}

const decoder = new TextDecoder("utf-8", { fatal: true });

export class LineReader {
  private pending: Buffer[] = [];
  private size = 0;

  constructor(private readonly maxBytes: number) {}

  /** Complete lines in arrival order; throws once a line exceeds `maxBytes` before its newline. */
  push(chunk: Buffer): string[] {
    const lines: string[] = [];
    let start = 0;
    for (let end = chunk.indexOf(0x0a); end !== -1; end = chunk.indexOf(0x0a, start)) {
      const part = chunk.subarray(start, end);
      if (this.size + part.length > this.maxBytes) throw new FrameTooLarge(`a frame exceeds ${this.maxBytes} bytes`);
      const line = this.pending.length ? Buffer.concat([...this.pending, part]) : part;
      this.pending = [];
      this.size = 0;
      try {
        lines.push(decoder.decode(line));
      } catch {
        throw new FrameInvalid("a frame is not valid UTF-8");
      }
      start = end + 1;
    }
    const rest = chunk.subarray(start);
    if (this.size + rest.length > this.maxBytes) throw new FrameTooLarge(`a frame exceeds ${this.maxBytes} bytes`);
    if (rest.length) {
      this.pending.push(Buffer.from(rest));
      this.size += rest.length;
    }
    return lines;
  }
}
