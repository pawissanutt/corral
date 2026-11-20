import { expect, test } from "bun:test";
import { cardDetail, questionEntries } from "./remote/cards.ts";
import { PendingQuestions, QUESTION_ENTRY, QUESTION_TTL, type QuestionEntry } from "./pending-questions.ts";
import { ToolApprovals } from "./tool-approvals.ts";

function fixture() {
  const entries: QuestionEntry[] = [];
  let now = 42;
  const questions = new PendingQuestions({ session: () => "session", entries: () => entries,
    append: data => { entries.push({ type: "custom", customType: QUESTION_ENTRY, data }); },
    changed() {}, deliver: async () => false, now: () => now });
  return { questions, approvals: new ToolApprovals(questions, "/session"), expire: () => { now += QUESTION_TTL; } };
}

const outside = { action: "ask", path: "/session/link", canonical: "/outside/saved.txt" } as const;

test("an owner write approval covers the exact contents and canonical destination once", async () => {
  const { questions, approvals } = fixture();
  const input = { path: "link", content: "original" };
  const request = () => approvals.write("write", input, outside);
  const first = request();
  const id = questions.pending()[0]!.id;
  expect(first).toContain(id);
  expect(request()).toBe(first);
  expect(questions.pending()).toHaveLength(1);
  const appCard = questionEntries(questions)[0]!;
  expect(cardDetail(appCard.card).questions[0]!.text).toContain(outside.canonical);
  await appCard.answer([{ options: ["allow"] }], false, async () => {});
  expect(approvals.write("write", { ...input, content: "changed" }, outside)).toContain("Waiting");
  expect(approvals.write("write", { content: input.content, path: input.path }, outside)).toBeUndefined();
  expect(request()).toContain("Waiting");
  const retry = questions.pending().at(-1)!;
  await questions.answer(retry.id, [{ option: "allow" }]);
  expect(approvals.write("write", input, { ...outside, canonical: "/outside/other.txt" })).toContain("destination changed");
  expect(approvals.write("write", input, { action: "deny", reason: "protected" })).toBe("protected");
});

test("a command approval cannot switch tools, survive restore, or be used after its deadline", async () => {
  const { questions, approvals, expire } = fixture();
  const input = { command: "touch marker" };
  expect(approvals.shell("bash", input)).toContain("Waiting");
  const id = questions.pending()[0]!.id;
  await questions.answer(id, [{ option: "allow" }]);
  expect(approvals.shell("job_start", input)).toContain("Waiting");
  expect(approvals.shell("bash", { ...input, timeout: 15.5 })).toContain("Waiting");
  expire();
  expect(approvals.shell("bash", input)).toContain("No current owner approval");
  expect(approvals.shell("bash", input)).toContain("Waiting");
  const beforeRestore = questions.pending()[0]!.id;
  approvals.clear();
  questions.restore();
  expect(questions.status(beforeRestore)).toBe("cancelled");
  expect(approvals.shell("bash", input)).not.toContain(beforeRestore);
});

test("approval is not offered if the app would truncate a command", () => {
  const { questions, approvals } = fixture();
  expect(approvals.shell("bash", { command: `echo '${"✨".repeat(350)}'` })).toContain("does not fit");
  expect(questions.pending()).toHaveLength(0);
});
