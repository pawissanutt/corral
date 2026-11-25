import { beforeAll, expect, test } from "bun:test";
import { initTheme } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { lineDiff, planSections, sectionChanges } from "./plan-doc.ts";
import { PlanView, type PlanViewInput, type PlanViewResult } from "./plan-view.ts";

beforeAll(() => initTheme("dark", false));
const PLAN = ["# Save slots", "", "Intro with **bold** words.", "", "## Storage", "", "- one file per slot", "", "```sh", "# not a heading", "```", "",
  "## Migration", "", ...Array.from({ length: 30 }, (_, index) => `Step ${index + 1}.\n`), "## Risks", "", "None known."].join("\n");
const ENTER = "\r";

function view(input: Partial<PlanViewInput> = {}, rows = 30) {
  const results: (PlanViewResult | undefined)[] = [];
  const plan = new PlanView({ title: "Save slots", revision: 2, model: "provider/model", text: PLAN, ...input }, () => rows, () => {}, result => results.push(result));
  const shown = (width = 120) => plan.render(width).map(line => stripTerminalSequences(line));
  return { plan, results, shown };
}

test("sections follow headings outside code fences, and a later submission names what changed", () => {
  const sections = planSections(PLAN);
  expect(sections.map(section => section.title)).toEqual(["Save slots", "Storage", "Migration", "Risks"]);
  // A heading that reads like another one's second occurrence is still a section of its own.
  expect(new Set(planSections("# A\n\n# A\n\n# A #2").map(section => section.key)).size).toBe(3);
  const next = planSections(PLAN.replace("- one file per slot", "- one directory per slot").replace("## Risks\n\nNone known.", "## Rollout\n\nBehind a flag."));
  const { changes, removed } = sectionChanges(sections, next);
  expect(changes).toEqual(["same", "changed", "same", "added"]);
  expect(removed.map(section => section.title)).toEqual(["Risks"]);
  expect(lineDiff("a\nb\nc", "a\nB\nc").map(line => line.kind + line.text)).toEqual([" a", "-b", "+B", " c"]);
});

test("the plan reads as rendered Markdown with its outline, and brackets step through its sections", () => {
  const { plan, shown } = view();
  const first = shown();
  for (const line of plan.render(120)) expect(visibleWidth(line)).toBe(120);
  expect(first.length).toBe(Math.floor(30 * 0.92));
  const text = first.join("\n");
  expect(text).toContain("Intro with bold words.");
  expect(text).not.toContain("**bold**");
  // The outline names every section beside the document; a narrow frame has none and still fits.
  expect(first.filter(line => line.includes("│ ") && /Migration|Risks/.test(line.slice(0, 34))).length).toBe(2);
  for (const line of plan.render(70)) expect(visibleWidth(line)).toBe(70);
  plan.handleInput("]"); plan.handleInput("]");
  expect(shown()[0]).toContain("§ 3/4");
  expect(shown()[3]).toContain("Migration");
  plan.handleInput("[");
  expect(shown()[0]).toContain("§ 2/4");
});

test("no single key approves: a digit selects, Enter carries it out, and Keep planning returns the marked headings", () => {
  const approve = view();
  approve.shown();
  approve.plan.handleInput(ENTER); approve.plan.handleInput("e");
  expect(approve.results).toEqual([]);
  approve.plan.handleInput("2"); approve.shown();
  expect(approve.results).toEqual([]);
  approve.plan.handleInput(ENTER);
  expect(approve.results).toEqual([{ choice: "execute_clean", quotes: [] }]);

  const revise = view();
  revise.shown();
  revise.plan.handleInput("]"); revise.shown(); revise.plan.handleInput("c");
  expect(revise.shown().join("\n")).toContain("marked for your feedback");
  revise.plan.handleInput("4"); revise.plan.handleInput(ENTER);
  expect(revise.results).toEqual([{ choice: "revise", quotes: ["Storage"] }]);
});

test("d shows what changed since the submission before: changed lines, new sections and removed ones", () => {
  const previous = PLAN.replace("- one file per slot", "- one row per slot").replace("## Risks", "## Open questions");
  const { plan, shown } = view({ previous });
  expect(shown().join("\n")).toContain("since revision 1: 1 changed, 1 new, 1 removed");
  plan.handleInput("d");
  const text = shown().join("\n");
  expect(text).toContain("- - one row per slot");
  expect(text).toContain("+ - one file per slot");
  expect(text).toContain("+ Risks · new section");
  expect(text).toContain("- Open questions · removed");
  expect(text).not.toContain("Step 12.");
});

test("a click only selects a choice, and a narrow frame still shows every choice and the whole model", () => {
  const { plan, results, shown } = view();
  const row = shown().findIndex(line => line.includes("Execute with clean context"));
  const click = () => plan.handleMouse({ type: "click", button: "left", x: 12, y: row, screenX: 12, screenY: row, width: 120, height: 27,
    shift: false, alt: false, ctrl: false, clickCount: 1 });
  click(); shown(); click();
  expect(results).toEqual([]);
  plan.handleInput(ENTER);
  expect(results).toEqual([{ choice: "execute_clean", quotes: [] }]);

  const narrow = view({}, 30);
  for (const line of narrow.plan.render(26)) expect(visibleWidth(line)).toBe(26);
  const text = narrow.shown(26).join("\n");
  for (const label of ["Execute", "Clean", "Compacted", "Keep planning", "provider/model"]) expect(text).toContain(label);

  // Too small to show the whole model above the choices: nothing executes, and Keep planning still works.
  const tiny = view({ model: "provider/" + "m".repeat(60) }, 12);
  tiny.shown(26);
  tiny.plan.handleInput("1"); tiny.plan.handleInput(ENTER);
  expect(tiny.results).toEqual([]);
  tiny.plan.handleInput("4"); tiny.plan.handleInput(ENTER);
  expect(tiny.results).toEqual([{ choice: "revise", quotes: [] }]);
});

test("an approved plan has no choice rows and ignores review-only keys", () => {
  const approved = view({ readonly: true });
  approved.shown();
  for (const key of ["1", "2", "3", ENTER, "c", "m"]) approved.plan.handleInput(key);
  expect(approved.results).toEqual([]);
  expect(approved.shown().join("\n")).not.toMatch(/Execute with|Keep planning|mark for feedback/);
  approved.plan.handleInput("]");
  expect(approved.shown()[0]).toContain("§ 2/4");
});
