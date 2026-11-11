// Model-family calibration: a short per-role overlay added after the canonical contract for
// model families whose observed defaults the contract alone does not counter. Authority and
// safety rules live only in the contract; an overlay adjusts working style and nothing else.
import { existsSync, lstatSync, readFileSync } from "node:fs";
import { join } from "node:path";

import type { Role } from "../types.ts";

export type PromptFamily = "gpt" | "claude" | "default";

const CLAUDE = /claude|opus|sonnet|haiku|fable|mythos/i;
const GPT = /gpt|codex/i;

/** The prompt family of a model: its ID decides first, then its provider. */
export function promptFamily(provider: string, model: string): PromptFamily {
  if (CLAUDE.test(model)) return "claude";
  if (GPT.test(model)) return "gpt";
  if (provider === "anthropic") return "claude";
  if (provider === "openai-codex") return "gpt";
  return "default";
}

export function calibrationAsset(role: Role, family: PromptFamily): string {
  return `assets/families/${family}/${role}.md`;
}

/**
 * The role's calibration reader. A family without an overlay file has no calibration; each
 * file is read once per role module.
 */
export function calibrationReader(root: string, role: Role): (provider: string, model: string) => string | undefined {
  const cache = new Map<PromptFamily, string | undefined>();
  return (provider, model) => {
    const family = promptFamily(provider, model);
    if (!cache.has(family)) {
      const path = join(root, calibrationAsset(role, family));
      cache.set(family, existsSync(path) && lstatSync(path).isFile() ? readFileSync(path, "utf8").trim() : undefined);
    }
    return cache.get(family);
  };
}
