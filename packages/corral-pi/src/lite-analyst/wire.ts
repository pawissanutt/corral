import type { Readable, Writable } from "node:stream";
import type { JsonObject, JsonValue } from "@earendil-works/pi-ai";

// The IPC envelope has its own bound; the provider request body has the smaller C1 bound.
export const MAX_FRAME_BYTES = 524_288;
export const MAX_BODY_BYTES = 262_144;

export function requireWire(condition: unknown): asserts condition {
  if (!condition) throw new Error("closed fixture protocol rejected input");
}

export function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function closedObject(value: unknown, ...keys: string[]): JsonObject {
  requireWire(isObject(value) && Object.keys(value).length === keys.length &&
    keys.every((key) => Object.hasOwn(value, key)));
  return value;
}

// Check lexical restrictions before native decoding can collapse duplicate keys, round values
// or accept lone surrogates. A JSON number in this fixture is a signed safe integer only.
export function parseStrictJson(text: string): JsonValue {
  let pos = 0;
  const whitespace = /[\x20\t\r\n]*/y;
  const scalar = /(?:null|true|false|-?(?:0|[1-9][0-9]*))/y;
  function skip() {
    whitespace.lastIndex = pos;
    whitespace.test(text);
    pos = whitespace.lastIndex;
  }
  function take(token: string) {
    skip();
    if (text[pos] !== token) return false;
    pos++;
    return true;
  }
  function string(): string {
    requireWire(take('"'));
    const start = pos - 1;
    while (pos < text.length) {
      const char = text[pos++];
      if (char === "\\") pos++;
      else if (char === '"') {
        const value: string = JSON.parse(text.slice(start, pos));
        requireWire(value.isWellFormed());
        return value;
      }
    }
    throw new Error("unterminated fixture JSON string");
  }
  function scan(depth: number) {
    requireWire(depth <= 32);
    if (take("{")) {
      if (take("}")) return;
      const keys = new Set<string>();
      do {
        const key = string();
        requireWire(!keys.has(key) && take(":"));
        keys.add(key);
        scan(depth + 1);
      } while (take(","));
      requireWire(take("}"));
    } else if (take("[")) {
      if (take("]")) return;
      do scan(depth + 1); while (take(","));
      requireWire(take("]"));
    } else if (text[pos] === '"') {
      string();
    } else {
      scalar.lastIndex = pos;
      const match = scalar.exec(text);
      requireWire(match);
      const token = match[0];
      if (token !== "null" && token !== "true" && token !== "false") {
        requireWire(Number.isSafeInteger(Number(token)));
      }
      pos = scalar.lastIndex;
    }
  }
  scan(0);
  skip();
  requireWire(pos === text.length);
  return JSON.parse(text) as JsonValue;
}

// RFC 8785 for this fixture's strict, safe-integer JSON domain. Array order is unchanged.
export function canonical(value: JsonValue): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (isObject(value)) {
    return `{${Object.keys(value).sort().map((key) =>
      `${JSON.stringify(key)}:${canonical(value[key]!)}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

async function* frames(input: Readable): AsyncGenerator<JsonValue> {
  const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
  const frame = Buffer.allocUnsafe(MAX_FRAME_BYTES);
  let length = 0;
  for await (const chunk of input) {
    requireWire(Buffer.isBuffer(chunk));
    let offset = 0;
    while (offset < chunk.length) {
      const lf = chunk.indexOf(0x0a, offset);
      const end = lf === -1 ? chunk.length : lf;
      requireWire(end - offset <= frame.length - length);
      chunk.copy(frame, length, offset, end);
      length += end - offset;
      if (lf === -1) break;
      requireWire(length > 0);
      const value = parseStrictJson(decoder.decode(frame.subarray(0, length)));
      length = 0;
      offset = lf + 1;
      yield value;
    }
  }
  requireWire(length === 0);
}

export class FixturePipe {
  private readonly input: AsyncGenerator<JsonValue>;

  constructor(input: Readable, private readonly output: Writable) {
    this.input = frames(input);
  }

  async receive(): Promise<JsonValue> {
    const next = await this.input.next();
    requireWire(!next.done);
    return next.value;
  }

  async send(message: object): Promise<void> {
    const text = JSON.stringify(message);
    requireWire(Buffer.byteLength(text) <= MAX_FRAME_BYTES);
    await new Promise<void>((resolve, reject) => {
      this.output.write(`${text}\n`, (error) => error ? reject(error) : resolve());
    });
  }
}
