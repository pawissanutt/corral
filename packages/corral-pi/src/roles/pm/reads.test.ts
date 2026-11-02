import { expect, test } from "bun:test";
import { Check } from "typebox/value";
import { pmTools } from "./tools.ts";

const show = pmTools.find((tool) => tool.name === "corral_show")!;
test("PM read schema offers exact-question and opt-in bounded pages without authority inputs", () => {
  const payload = { task: "T-337", section: "questions", key: "T-337/R1/Q16", limit: 4096 };
  expect(Check(show.parameters, payload)).toBeTrue();
  expect(show.payload(payload)).toEqual(payload);
  expect(Check(show.parameters, { task: "T-337" })).toBeTrue();
  expect(Check(show.parameters, { task: "T-664", section: "automation" })).toBeTrue();
  for (const extra of [{ limit: 4097 }, { offset: -1 }, { repair: true }, { project: "other" }]) {
    expect(Check(show.parameters, { task: "T-337", ...extra })).toBeFalse();
  }
});
