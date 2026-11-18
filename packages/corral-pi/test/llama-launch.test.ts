import { expect, test } from "bun:test";

import { parseRunFlags } from "../src/host/config.ts";

test("local provider flags accept a newly named model", () => {
  const config = parseRunFlags("run", ["--dir", "r", "--provider", "llama.cpp", "--model", "new-family/new-model.gguf"], "/assets", {}, "/fixture");
  expect(config).toMatchObject({ provider: "llama.cpp", model: "new-family/new-model.gguf" });
});
