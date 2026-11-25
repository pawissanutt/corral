import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ImageContent } from "@earendil-works/pi-ai";
import { ComposerPaste, PASTE_END, PASTE_START, StagedImages, pastedImagePaths, type PasteBase } from "./composer-attach.ts";

const PNG_1X1 = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "base64");

function fakeBase(text: string) {
  const calls = { input: [] as string[], insert: [] as string[], clipboard: 0 };
  const base: PasteBase = {
    input: data => { calls.input.push(data); }, insert: data => { calls.insert.push(data); }, clipboard: () => { calls.clipboard += 1; },
    text: () => text, cursor: () => ({ line: 0, col: text.length }), lines: () => [text],
  };
  return { base, calls };
}
const paste = (images: StagedImages) => new ComposerPaste({
  images, imageInput: () => ({ accepts: true, model: "m" }), notice: () => {}, pasteKeys: () => ["ctrl+v"],
});

test("a dropped image path becomes an image at the cursor", () => {
  const path = join(mkdtempSync(join(tmpdir(), "corral-attach-")), "shot one.png");
  writeFileSync(path, PNG_1X1);
  const images = new StagedImages(), { base, calls } = fakeBase("look at ");
  expect(paste(images).handle(PASTE_START + "'" + path + "'" + PASTE_END, base)).toBe(true);
  expect(calls.insert).toEqual(["[Image #1]"]);
  expect(calls.input).toEqual([]);
  images.claim("look at [Image #1]");
  const taken = images.take("look at [Image #1]");
  expect(taken.images.map(image => image.mimeType)).toEqual(["image/png"]);
});

test("attaching never deletes the file it read, even one named like Pi's clipboard file", () => {
  const path = join(tmpdir(), `pi-clipboard-${crypto.randomUUID()}.png`);
  writeFileSync(path, PNG_1X1);
  try {
    const router = paste(new StagedImages()), { base } = fakeBase("");
    router.handle("\x16", base);
    expect(router.inserted(path, base)).toBe(true);
    expect(existsSync(path)).toBe(true);
  } finally { rmSync(path, { force: true }); }
});

test("a message Pi sends first leaves the images of the draft still being written", () => {
  const images = new StagedImages();
  const image = (data: string): ImageContent => ({ type: "image", data, mimeType: "image/png" });
  images.stage(image("first"), "first"); images.stage(image("second"), "second");
  // Held during compaction, the earlier text-only message arrives while the draft marks image 2.
  expect(images.take("held words", "look at [Image #2]")).toMatchObject({ images: [], dropped: 1 });
  images.claim("look at [Image #2]");
  expect(images.take("look at [Image #2]").images.map(image => image.data)).toEqual(["second"]);
});

test("an image cleared from the draft is never sent, even when a pasted text names its marker", () => {
  const images = new StagedImages();
  images.stage({ type: "image", data: "discarded", mimeType: "image/png" }, "shot");
  // Ctrl+C cleared `[Image #1]`; the new draft is one folded paste whose text happens to say `[Image #1]`.
  images.claim("[paste #1 +20 lines]");
  expect(images.count).toBe(0);
  const taken = images.take("the syntax is [Image #1] here");
  expect(taken).toMatchObject({ images: [], text: "the syntax is [Image #1] here", dropped: 1 });
});

test("images whose markers were cleared do not count toward the per-message limit", () => {
  const path = join(mkdtempSync(join(tmpdir(), "corral-attach-")), "shot.png");
  writeFileSync(path, PNG_1X1);
  const images = new StagedImages();
  for (let at = 0; at < 8; at++) images.stage({ type: "image", data: "x", mimeType: "image/png" }, "old");
  // The draft that marked all eight was cleared (Ctrl+C); the next image is the new draft's first.
  const { base, calls } = fakeBase("");
  expect(paste(images).handle(PASTE_START + path + PASTE_END, base)).toBe(true);
  expect(calls.insert).toEqual(["[Image #9]"]);
});

test("clipboard text over the fold limit goes through pi-tui's paste fold", () => {
  const router = paste(new StagedImages()), { base, calls } = fakeBase("");
  expect(router.handle("\x16", base)).toBe(false);
  const text = Array.from({ length: 50 }, (_, at) => `line ${at}`).join("\n");
  expect(router.inserted(text, base)).toBe(true);
  expect(calls.input).toEqual([PASTE_START + text + PASTE_END]);
  // Text typed without the paste key is inserted as it is.
  expect(router.inserted(text, base)).toBe(false);
});

test("only text that is nothing but image paths names images", () => {
  expect(pastedImagePaths("C:\\Users\\me\\shot.png", { WSL_DISTRO_NAME: "x" })).toEqual(["/mnt/c/Users/me/shot.png"]);
  expect(pastedImagePaths("file:///tmp/a%20b.png", {})).toEqual(["/tmp/a b.png"]);
  expect(pastedImagePaths("see /tmp/x.png", {})).toBeUndefined();
  expect(pastedImagePaths("/tmp/notes.txt", {})).toBeUndefined();
});

test("removed markers are not sent and the rest renumber", () => {
  const images = new StagedImages();
  const image = (data: string): ImageContent => ({ type: "image", data, mimeType: "image/png" });
  for (const data of ["first", "second", "third"]) images.stage(image(data), data);
  images.claim("[Image #3] and [Image #1]");
  const taken = images.take("[Image #3] and [Image #1]");
  expect(taken.images.map(image => image.data)).toEqual(["third", "first"]);
  expect(taken.text).toBe("[Image #1] and [Image #2]");
  expect(taken.dropped).toBe(1);
  expect(images.count).toBe(0);
});

test("a queued message pulled back to the draft keeps its images", () => {
  const images = new StagedImages();
  const image = (data: string): ImageContent => ({ type: "image", data, mimeType: "image/png" });
  images.stage(image("draft"), "draft");
  const text = images.restage("see [Image #2] then [Image #1]", [image("first"), image("second")]);
  expect(text).toBe("see [Image #2] then [Image #3]");
  images.claim(`[Image #1] ${text}`);
  const taken = images.take(`[Image #1] ${text}`);
  expect(taken.images.map(image => image.data)).toEqual(["draft", "second", "first"]);
  expect(taken.dropped).toBe(0);
});
