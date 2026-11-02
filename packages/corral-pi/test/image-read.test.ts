import { expect, test } from "bun:test";
import { createReadTool } from "../node_modules/@earendil-works/pi-coding-agent/dist/core/tools/read.js";
import { fileURLToPath } from "node:url";
import { statSync } from "node:fs";

const image = fileURLToPath(new URL("./fixtures/images/read-screen.png", import.meta.url));

test("Pi read delivers a 1288x990 210 KiB screenshot as an image part", async () => {
  expect(statSync(image).size).toBeGreaterThan(210_000);
  const result = await createReadTool(process.cwd()).execute("read", { path: image });
  const part = result.content.find(block => block.type === "image");
  expect(part?.type).toBe("image");
  if (part?.type === "image") expect(part.mimeType).toBe("image/png");
});

test("an undeliverable image reports the encoded size and limit", async () => {
  const result = await createReadTool(process.cwd(), { resizeOptions: { maxBytes: 1 } }).execute("read", { path: image });
  expect(result.content.some(block => block.type === "image")).toBe(false);
  const text = result.content.find(block => block.type === "text");
  expect(text?.type === "text" && text.text).toContain("inline base64 limit 1 bytes");
  expect(text?.type === "text" && text.text).toContain("original 215586 bytes");
});
