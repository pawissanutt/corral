import { expect, test } from "bun:test";
import { Errors } from "typebox/value";
import { artifactTool } from "./tools.ts";

test("artifact requires one of the three public kinds", () => {
  for (const kind of ["reference", "report", "evidence"]) {
    const params = { path: "proof.txt", kind };
    expect(Errors(artifactTool.parameters, params)).toHaveLength(0);
    expect(artifactTool.payload(params)).toEqual(params);
  }
  for (const params of [{ path: "proof.txt" }, { path: "proof.txt", kind: "log" }, { path: "proof.txt", kind: "other" }]) {
    expect(Errors(artifactTool.parameters, params).length).toBeGreaterThan(0);
  }
});
