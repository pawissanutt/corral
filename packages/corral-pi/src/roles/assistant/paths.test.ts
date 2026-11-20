import { expect, test } from "bun:test";
import { createWriteTool } from "@earendil-works/pi-coding-agent";
import { resolveToolPath } from "./paths.ts";

test("the guard and Pi's write tool resolve the same target", async () => {
  let sdkPath: string | undefined;
  const cwd = "/fixture";
  const write = createWriteTool(cwd, { operations: {
    mkdir: async () => {}, writeFile: async path => { sdkPath = path; },
  } });
  for (const path of ["/abs/../b", "a\u00A0b", "a\u1680b", "@./file", "~/file", "file:///fixture/a%20b", " ../literal-space"]) {
    await write.execute("fixture", { path, content: "" });
    expect(resolveToolPath(path, cwd)).toBe(sdkPath!);
  }
});
