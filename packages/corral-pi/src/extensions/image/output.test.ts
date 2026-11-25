import { expect, test } from "bun:test";
import { lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, renameSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { crc32 } from "node:zlib";
import { preparePng, validatePng } from "./output.ts";

const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
const bytes = Buffer.from(png, "base64");
const fsTest = test.skipIf(process.platform !== "linux");

function fixture(run: (root: string, outside: string) => void): void {
  const base = mkdtempSync(join(tmpdir(), "corral-image-"));
  try {
    const root = join(base, "root"), outside = join(base, "outside");
    mkdirSync(root);
    mkdirSync(outside);
    run(root, outside);
  } finally { rmSync(base, { force: true, recursive: true }); }
}

test("accepts canonical PNG and rejects invalid encodings, truncation, corruption and excess size", () => {
  expect(validatePng(png)).toEqual(bytes);
  for (const invalid of [png.slice(0, -2), `${png}\n`, png.replace("g==", "h=="), "YQ==",
    bytes.subarray(0, -1).toString("base64"), Buffer.concat([bytes, bytes]).toString("base64")]) {
    expect(() => validatePng(invalid)).toThrow();
  }
  expect(() => validatePng(Buffer.alloc(4 * 1024 * 1024 + 1).toString("base64"))).toThrow("4 MiB");
  const corrupt = Buffer.from(bytes);
  corrupt[44] = corrupt[44]! ^ 1;
  expect(() => validatePng(corrupt.toString("base64"))).toThrow("PNG");
  const oversized = Buffer.from(bytes);
  oversized.writeUInt32BE(4097, 16);
  oversized.writeUInt32BE(crc32(oversized.subarray(12, 29)), 29);
  expect(() => validatePng(oversized.toString("base64"))).toThrow("dimensions");
});

fsTest("publishes exact bytes exclusively with private mode and leaves no stage", () => fixture(root => {
  const out = preparePng(root, "image.png");
  try {
    expect(readdirSync(root)).toEqual([]);
    out.publish(validatePng(png));
    expect(() => out.publish(bytes)).toThrow();
  } finally { out.close(); out.close(); }
  expect(readdirSync(root)).toEqual(["image.png"]);
  const file = join(root, "image.png");
  expect(readFileSync(file)).toEqual(bytes);
  expect(lstatSync(file).mode & 0o077).toBe(0);
  expect(() => preparePng(root, "image.png")).toThrow("already exists");
}));

fsTest("refuses traversal, private components, symlinks and missing directories before publication", () => fixture((root, outside) => {
  symlinkSync(outside, join(root, "redirect"));
  symlinkSync(join(outside, "missing"), join(root, "dangling.png"));
  mkdirSync(join(root, ".git"));
  mkdirSync(join(root, ".corral-next"));
  for (const path of ["../escape.png", "/absolute.png", "C:\\absolute.png", "a/../escape.png", ".git/image.png", ".corral-next/image.png", ".worktrees/image.png", "redirect/image.png", "dangling.png", "missing/image.png"]) {
    expect(() => preparePng(root, path)).toThrow();
  }
  expect(() => preparePng(join(root, "redirect"), "image.png")).toThrow();
  expect(readdirSync(outside)).toEqual([]);
}));

fsTest("refuses a late entrant and leaves it unchanged; close and invalid bytes leave no file", () => fixture(root => {
  const out = preparePng(root, "taken.png");
  try {
    writeFileSync(join(root, "taken.png"), "winner");
    expect(() => out.publish(bytes)).toThrow("already exists");
  } finally { out.close(); }
  const invalid = preparePng(root, "invalid.png");
  try { expect(() => invalid.publish(Buffer.from("not png"))).toThrow("PNG"); }
  finally { invalid.close(); }
  const cancelled = preparePng(root, "cancelled.png");
  cancelled.close();
  expect(() => cancelled.publish(bytes)).toThrow();
  expect(readFileSync(join(root, "taken.png"), "utf8")).toBe("winner");
  expect(readdirSync(root)).toEqual(["taken.png"]);
}));

fsTest("refuses parent or root relocation while the provider is running", () => fixture((root, outside) => {
  const parent = join(root, "dir");
  mkdirSync(parent);
  const out = preparePng(root, "dir/image.png");
  try {
    renameSync(parent, join(outside, "dir"));
    mkdirSync(parent);
    expect(() => out.publish(bytes)).toThrow("changed");
  } finally { out.close(); }
  expect(readdirSync(parent)).toEqual([]);
  expect(readdirSync(join(outside, "dir"))).toEqual([]);
  const movedRoot = preparePng(root, "image.png");
  try {
    renameSync(root, join(outside, "root"));
    mkdirSync(root);
    expect(() => movedRoot.publish(bytes)).toThrow("changed");
  } finally { movedRoot.close(); }
  expect(readdirSync(root)).toEqual([]);
  expect(readdirSync(join(outside, "root"))).toEqual(["dir"]);
}));
