import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

import { PACKAGE_ROOT } from "./support/fixture-install.ts";

const REPO = join(PACKAGE_ROOT, "../..");
type Rates = { input: number; output: number; cache_read: number; cache_write: number };
type Entry = { id: string; type: string; cost: { input: number; output: number; cacheRead: number; cacheWrite: number } };

const piAiRoot = dirname(createRequire(join(PACKAGE_ROOT, "package.json")).resolve("@earendil-works/pi-ai/package.json"));
const catalog = JSON.parse(readFileSync(join(piAiRoot, "dist/providers/data/amazon-bedrock.json"), "utf8"))["bedrock-converse-stream"] as Record<string, Entry>;
const frozen = JSON.parse(readFileSync(join(REPO, "crates/corral-core/src/pricing_bedrock.json"), "utf8")) as Record<string, Rates>;

test("frozen Bedrock rates equal pi-ai's catalog for every chat ID, with no extra IDs", () => {
  const chat = Object.values(catalog).filter((m) => m.type === "chat");
  expect(chat.length).toBeGreaterThan(0);
  expect(Object.keys(frozen).sort()).toEqual(chat.map((m) => m.id).sort());
  for (const m of chat) {
    expect([m.id, frozen[m.id]]).toEqual([m.id, {
      input: m.cost.input, output: m.cost.output, cache_read: m.cost.cacheRead, cache_write: m.cost.cacheWrite,
    }]);
  }
});
