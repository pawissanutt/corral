import { expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseSubmitInput } from "../bridge/protocol.ts";
import { sha256Hex } from "../tools/corral.ts";
import { loadOwnerAttachments, type OwnerAttachment } from "./owner-attachments.ts";

test("an image requires model support and is passed to Pi with its validated bytes", () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-image-"));
  try {
    mkdirSync(join(dir, "attachments"));
    const data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
    const bytes = Buffer.from(data, "base64");
    const image: OwnerAttachment = { id: "a1", kind: "image", name: "shot.png", media_type: "image/png",
      bytes: bytes.length, sha256: sha256Hex(bytes) };
    writeFileSync(join(dir, "attachments", image.sha256!), bytes);
    expect(() => loadOwnerAttachments(dir, [image], { id: "fixture", input: ["text"] })).toThrow("attachment_unsupported");
    const loaded = loadOwnerAttachments(dir, [image], { id: "fixture", input: ["text", "image"] });
    expect(loaded.images).toEqual([{ type: "image", mimeType: "image/png", data }]);
    expect(loaded.text).toContain("[Untrusted attachments");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("file attachments reject tampering and symlinks and fence accepted UTF-8", () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-file-"));
  try {
    mkdirSync(join(dir, "attachments"));
    const text = "untrusted text";
    const hash = sha256Hex(text);
    const file: OwnerAttachment = { id: "a1", kind: "file", name: "log.txt", bytes: Buffer.byteLength(text), sha256: hash };
    const path = join(dir, "attachments", hash);
    writeFileSync(path, "x".repeat(text.length));
    expect(() => loadOwnerAttachments(dir, [file], undefined)).toThrow("bad_attachment");
    unlinkSync(path);
    const outside = join(dir, "other.txt");
    writeFileSync(outside, text);
    symlinkSync(outside, path);
    expect(() => loadOwnerAttachments(dir, [file], undefined)).toThrow("bad_attachment");
    unlinkSync(path);
    writeFileSync(path, text);
    const loaded = loadOwnerAttachments(dir, [file], undefined);
    expect(loaded.images).toEqual([]);
    expect(loaded.text).toContain(`<attachment a1 sha256:${hash}>\n${text}\n</attachment a1 sha256:${hash}>`);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("socket attachments are bounded metadata, never caller-selected host paths", () => {
  const file = { id: "a1", kind: "file", name: "log.txt", bytes: 1, sha256: "a".repeat(64) };
  for (const attachments of [
    [file, file], Array.from({ length: 5 }, () => file), [{ ...file, path: "/host/file" }],
    [{ ...file, bytes: 65537 }], [{ ...file, sha256: "../escape" }], [{ ...file, name: "bad\nname" }],
    [{ id: "a1", kind: "link", name: "title", url: "file:///host/file" }],
    [{ id: "a1", kind: "link", name: "title", url: "https://user:password@example.com" }],
  ]) expect(() => parseSubmitInput({ origin: "owner", text: "go", attachments })).toThrow("bad_attachment");
  expect(parseSubmitInput({ origin: "owner", text: "go", attachments: [file] }).attachments).toEqual([file]);
});
