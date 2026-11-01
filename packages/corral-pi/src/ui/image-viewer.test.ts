import { test, expect } from "bun:test";
import { readFileSync } from "node:fs";
import { NativeWorkbench } from "./workbench.ts";
import { Text } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
import { Image, getCapabilities, setCapabilities, getCellDimensions, setCellDimensions, setImageActivationHandler, type TuiMouseEvent } from "@earendil-works/pi-tui";
const data = readFileSync(new URL("../../test/fixtures/images/zoom.png", import.meta.url)).toString("base64");
const click: TuiMouseEvent = { type: "click", button: "left", x: 0, y: 0, screenX: 0, screenY: 0, width: 60, height: 20, shift: false, alt: false, ctrl: false, clickCount: 1 };
test("image activation is scoped and removable; modified/double clicks retain native behavior", () => {
  let count = 0;
  const remove = setImageActivationHandler(source => { expect(source.data).toBe(data); expect(source.mimeType).toBe("image/png"); count++; });
  const image = new Image(data, "image/png", { fallbackColor: text => text });
  try {
    expect(image.handleMouse(click)?.handled).toBeTrue(); expect(count).toBe(1);
    expect(image.handleMouse({ ...click, shift: true })).toBeUndefined();
    expect(image.handleMouse({ ...click, clickCount: 2 })).toBeUndefined();
    const deny = setImageActivationHandler(() => false);
    expect(image.handleMouse(click)).toBeUndefined(); deny();
    expect(image.handleMouse(click)?.handled).toBeTrue(); expect(count).toBe(2);
  } finally { remove(); }
  expect(image.handleMouse(click)).toBeUndefined();
});
test("expanded image respects real dock viewport, retains draft and restores transcript on Escape", () => {
  const caps = getCapabilities(); setCapabilities({ ...caps, images: "kitty" });
  const wb = new NativeWorkbench({ cwd: "/tmp", identity: { store: "s", project: "p", role: "assistant", assistant: "A-test", generation: 1 }, snapshot: () => null, changed() {}, canInteract: () => true });
  try {
    const viewport = createChatViewport({ document: new Text("ORIGINAL_TRANSCRIPT", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0), editor: new Text(Array(9).fill("REAL_DRAFT").join("\n"), 0, 0), footer: new Text("FOOTER", 0, 0), mainView: transcript => wb.mount(transcript) });
    wb.images.activate({ data, mimeType: "image/png", dimensions: { widthPx: 960, heightPx: 480 } });
    const screen = renderLayoutFrame(viewport.root, 80, 14, () => {}).lines.join("\n");
    expect(screen).toContain("REAL_DRAFT"); expect(screen).not.toContain("ORIGINAL_TRANSCRIPT");
    const rows = [...screen.matchAll(/\x1b_G[^;\x1b]*?r=(\d+)/g)].map(match => Number(match[1]));
    expect(rows.length).toBe(1); expect(rows[0]).toBeLessThanOrEqual(1);
    expect(wb.handleInput("\x1b")).toBeTrue(); expect(wb.images.isOpen).toBeFalse();
    expect(renderLayoutFrame(viewport.root, 80, 24, () => {}).lines.join("\n")).toContain("ORIGINAL_TRANSCRIPT");
  } finally { wb.dispose(); setCapabilities(caps); }
});
test("native Kitty image protocol uses larger bounded geometry for expanded image", () => {
  const capabilities = getCapabilities(), cells = getCellDimensions();
  try {
    setCapabilities({ ...capabilities, images: "kitty" }); setCellDimensions({ widthPx: 8, heightPx: 16 });
    const small = new Image(data, "image/png", { fallbackColor: text => text }, { maxWidthCells: 20, maxHeightCells: 8 });
    const large = new Image(data, "image/png", { fallbackColor: text => text }, { maxWidthCells: 75, maxHeightCells: 30 });
    const a = small.render(80), b = large.render(80);
    expect(a.join("")).toContain("\x1b_G"); expect(b.join("")).toContain("\x1b_G");
    expect(b.length).toBeGreaterThan(a.length); expect(b.length).toBeLessThanOrEqual(30);
    expect(small.getImageId()).toBeDefined(); expect(large.getImageId()).toBeDefined();
  } finally { setCapabilities(capabilities); setCellDimensions(cells); }
});
