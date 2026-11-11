import { expect, test } from "bun:test";

import { PACKAGE_ROOT } from "../../../test/support/fixture-install.ts";
import { calibrationReader, promptFamily } from "./family.ts";

test("a model's ID decides its prompt family before its provider", () => {
  expect(promptFamily("openai-codex", "gpt-6-astra")).toBe("gpt");
  expect(promptFamily("openai-codex", "gpt-6.1-sol")).toBe("gpt");
  expect(promptFamily("anthropic", "claude-opus-5-5")).toBe("claude");
  expect(promptFamily("amazon-bedrock", "us.anthropic.claude-fable-5-1")).toBe("claude");
  expect(promptFamily("openai-codex", "unlisted")).toBe("gpt");
  expect(promptFamily("llama.cpp", "qwen")).toBe("default");
});

test("only a family with a shipped overlay gets a calibration section", () => {
  for (const role of ["assistant", "pm"] as const) {
    const read = calibrationReader(PACKAGE_ROOT, role);
    expect(read("openai-codex", "gpt-6-astra")).toMatch(/^## Working style/);
    expect(read("anthropic", "claude-opus-5-5")).toBeUndefined();
    expect(read("llama.cpp", "qwen")).toBeUndefined();
  }
  expect(calibrationReader(PACKAGE_ROOT, "worker")("openai-codex", "gpt-6-astra")).toBeUndefined();
});
