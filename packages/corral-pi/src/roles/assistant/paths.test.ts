import { expect, test } from "bun:test";
import { resolveToolPath } from "./paths.ts";

test("tool paths resolve like Pi's edit and write tools", () => {
  expect(resolveToolPath("/abs/../b", "/w")).toBe("/b");
  expect(resolveToolPath("a\u00A0b", "/w")).toBe("/w/a b");
});
