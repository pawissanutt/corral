import { expect, test } from "bun:test";

import { FrameInvalid, FrameTooLarge, LineReader } from "../src/bridge/framing.ts";

const frame = (text: string) => `${JSON.stringify({ version: 1, id: "rq-1", command: "submit_input", args: { text } })}\n`;

test("a multibyte character split at every byte boundary reassembles exactly", () => {
  for (const text of ["A\u20acB", "x\u{1F600}y", "\u00e9\u00e8 \u4e2d\u6587"]) {
    const bytes = Buffer.from(frame(text));
    for (let cut = 1; cut < bytes.length; cut++) {
      const reader = new LineReader(1024);
      const lines = [...reader.push(bytes.subarray(0, cut)), ...reader.push(bytes.subarray(cut))];
      expect(lines).toHaveLength(1);
      expect(JSON.parse(lines[0] ?? "").args.text).toBe(text);
    }
    const reader = new LineReader(1024);
    const byteByByte = [...bytes].flatMap((byte) => reader.push(Buffer.from([byte])));
    expect(JSON.parse(byteByByte[0] ?? "").args.text).toBe(text);
  }
});

test("several frames in one chunk come out in order, with the trailing partial kept", () => {
  const reader = new LineReader(1024);
  const chunk = Buffer.from(`${frame("one")}${frame("tw\u20ac")}${frame("three").slice(0, 10)}`);
  expect(reader.push(chunk).map((line) => JSON.parse(line).args.text)).toEqual(["one", "tw\u20ac"]);
  expect(reader.push(Buffer.from(frame("three").slice(10))).map((line) => JSON.parse(line).args.text)).toEqual(["three"]);
});

test("invalid UTF-8 and a frame past the byte bound are refused, counted in wire bytes", () => {
  expect(() => new LineReader(1024).push(Buffer.from([0x7b, 0xe2, 0x82, 0x7d, 0x0a]))).toThrow(FrameInvalid);
  const bound = new LineReader(8);
  expect(bound.push(Buffer.from("\u20ac\u20ac"))).toEqual([]);
  expect(() => bound.push(Buffer.from("\u20ac"))).toThrow(FrameTooLarge);
  expect(new LineReader(6).push(Buffer.from("\u20ac\u20ac\n"))).toEqual(["\u20ac\u20ac"]);
});
