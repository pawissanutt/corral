import { expect, test } from "bun:test";
import { PanelCanvas } from "./panel-canvas.ts";

const enter = "\x1b[?1049h", exit = "\x1b[?1049l";
/** The one background of the whole screen, dark appearance: `#191b20`. */
const fg = "\x1b[38;2;224;227;233m", bg = "\x1b[48;2;25;27;32m";
function active() { const canvas = new PanelCanvas(); canvas.write(enter); return canvas; }

test("default colors are explicit only within the alternate screen; normal screen/palette bytes remain untouched", () => {
  const canvas = new PanelCanvas(), normal = "\x1b[0m\x1b]11;?\x07shell\x1b[39m";
  expect(canvas.write(normal).data).toBe(normal);
  expect(canvas.write(enter + "\x1b[2J").data).toBe(enter + fg + bg + "\x1b[2J");
  expect(canvas.write("\x1b[0mtext\x1b[39m\x1b[49m").data).toBe("\x1b[0m" + fg + bg + "text\x1b[39m" + fg + "\x1b[49m" + bg);
  expect(canvas.write(exit + normal).data).toBe(exit + normal);
  expect(canvas.write(enter).data).toBe(enter + fg + bg);
});
test("combined/default SGR preserves text attributes, explicit colors and RGB/index values resembling resets", () => {
  const canvas = active();
  for (const sgr of ["\x1b[38;2;0;39;49m", "\x1b[48;5;0m", "\x1b[38:2::0:39:49m", "\x1b[58;2;0;39;49m", "\x1b[31;44m", "\x1b[22m"]) {
    expect(canvas.write(sgr).data).toBe(sgr);
  }
  expect(canvas.write("\x1b[m").data).toBe("\x1b[m" + fg + bg);
  expect(canvas.write("\x1b[0;1m").data).toBe("\x1b[0;1m" + fg + bg);
  expect(canvas.write("\x1b[0;38;2;0;39;49m").data).toBe("\x1b[0;38;2;0;39;49m" + bg);
  expect(canvas.write("\x1b[0;48:2::0:39:49m").data).toBe("\x1b[0;48:2::0:39:49m" + fg);
  expect(canvas.write("\x1b[39;31m").data).toBe("\x1b[39;31m");
  expect(canvas.write("\x1b[31;39m").data).toBe("\x1b[31;39m" + fg);
});
test("OSC/DCS/APC payloads cannot inject mode changes or have their embedded SGR rewritten", () => {
  const canvas = active();
  for (const payload of ["\x1b]8;;https://example.invalid/" + exit + "\x1b[0m\x07", "\x1bP" + exit + "\x1b[39m\x1b\\", "\x1b_G" + exit + "\x1b[49m\x1b\\"]) {
    const result = canvas.write(payload);
    expect(result.data).toBe(payload); expect(result.screen).toBeUndefined();
    expect(canvas.atBoundary).toBeTrue();
  }
  expect(canvas.write("\x1b[49m").data).toBe("\x1b[49m" + bg);
});
test("streaming boundaries preserve opaque data and match unsplit rendering", () => {
  const data = enter + "\x1b[?7l\x1b[2J\x1b(Bhello 界\x1b#8\x1b[0;1m" +
    "\x1b]8;;https://example.invalid/\x1b[49m\x1b\\label\x1b]8;;\x1b\\" +
    "\x1b_Gabc\x1b[?1049l\x1b\\\x1b[39m" + exit + "\x1b[0mshell";
  const expected = new PanelCanvas().write(data).data;
  for (let split = 0; split <= data.length; split++) {
    const canvas = new PanelCanvas();
    expect(canvas.write(data.slice(0, split)).data + canvas.write(data.slice(split)).data).toBe(expected);
    expect(canvas.atBoundary).toBeTrue();
  }
  const canvas = new PanelCanvas();
  canvas.write(enter + "\x1b]52;c;partial"); expect(canvas.atBoundary).toBeFalse();
  canvas.write("\x07"); expect(canvas.atBoundary).toBeTrue();
});
test("ESC intermediate sequences and cancellation retain streaming boundaries", () => {
  const canvas = active();
  for (const prefix of ["\x1b(", "\x1b#", "\x1b%"]) {
    expect(canvas.write(prefix).data).toBe(prefix); expect(canvas.atBoundary).toBeFalse();
    expect(canvas.write("B").data).toBe("B"); expect(canvas.atBoundary).toBeTrue();
  }
  canvas.write("\x1b("); canvas.write("\x18"); expect(canvas.atBoundary).toBeTrue();
  canvas.write("\x1b]partial"); canvas.write("\x1a"); expect(canvas.atBoundary).toBeTrue();
});
test("oversized/malformed CSI metadata is bounded and ordinary output is preserved", () => {
  const canvas = active(), huge = "\x1b[" + "1;".repeat(300) + "m";
  expect(canvas.write(huge).data).toBe(huge);
  expect(canvas.atBoundary).toBeTrue();
  expect(canvas.write("\x1b[38;2m").data).toBe("\x1b[38;2m");
  canvas.reset(); expect(canvas.write("\x1b[0m").data).toBe("\x1b[0m");
});
