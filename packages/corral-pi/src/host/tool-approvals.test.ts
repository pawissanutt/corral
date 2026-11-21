import { expect, test } from "bun:test";
import { cardDetail, questionEntries } from "./remote/cards.ts";
import { APPROVAL_CHOICES as CHOICE, PendingQuestions, QUESTION_ENTRY, QUESTION_TTL, type QuestionEntry } from "./pending-questions.ts";
import { APPROVAL_TTL, ToolApprovals } from "./tool-approvals.ts";

function fixture(reusable = false) {
  const entries: QuestionEntry[] = [];
  let now = 42;
  const questions = new PendingQuestions({ session: () => "session", entries: () => entries,
    append: data => { entries.push({ type: "custom", customType: QUESTION_ENTRY, data }); },
    changed: question => { if (question) approvals.observeAnswer(question); },
    deliver: async () => false, now: () => now });
  const approvals = new ToolApprovals(questions, "/session", reusable, () => now);
  return { questions, approvals, advance: (ms: number) => { now += ms; }, expire: () => { now += QUESTION_TTL; } };
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
  expect(approvals.shell("bash", { ...input, timeout: 15.5, wait_ms: 0 })).toContain("Waiting");
  const prompt = questions.pending().at(-1)!.questions[0]!.question;
  expect(prompt).toContain("15.5 s");
  expect(prompt).toContain("background");
  expire();
  expect(approvals.shell("bash", input)).toContain("No current owner approval");
  expect(approvals.shell("bash", input)).toContain("Waiting");
  const beforeRestore = questions.pending()[0]!.id;
  approvals.clear();
  questions.restore();
  expect(questions.status(beforeRestore)).toBe("cancelled");
  expect(approvals.shell("bash", input)).not.toContain(beforeRestore);
});

test("a timed grant covers only the exact shell invocation and expires from the time of the answer", async () => {
  const { questions, approvals, advance } = fixture(true);
  const input = { command: "touch marker", wait_ms: 0 };
  expect(approvals.shell("bash", input)).toContain("Waiting");
  const [card] = questionEntries(questions);
  advance(5 * 60 * 1000);
  await card!.answer([{ options: [CHOICE.timed] }], false, async () => {});
  advance(APPROVAL_TTL - 1);
  expect(approvals.shell("bash", input)).toBeUndefined();
  expect(approvals.shell("bash", input)).toBeUndefined();
  expect(approvals.shell("job_start", input)).toContain("Waiting");
  expect(approvals.shell("bash", { ...input, wait_ms: 100 })).toContain("Waiting");
  advance(1);
  expect(approvals.shell("bash", input)).toContain("Waiting");
});

test("one file cannot have conflicting pending choices, and denied writes stay blocked on retry", async () => {
  const { questions, approvals } = fixture(true);
  const request = (content: string) => approvals.write("write", { path: "link", content }, outside);
  expect(request("first")).toContain("Waiting");
  const [card] = questions.pending();
  expect(request("second")).toContain(card!.id);
  expect(questions.pending()).toHaveLength(1);
  await questions.answer(card!.id, [{ option: CHOICE.deny }]);
  expect(request("first")).toContain("No current owner approval");
  expect(request("first")).toContain("Waiting");
  expect(questions.pending()[0]!.id).not.toBe(card!.id);
});

test("a session file grant is limited to the canonical destination and never restored", async () => {
  const { questions, approvals, advance } = fixture(true);
  expect(approvals.write("write", { path: "link", content: "first" }, outside)).toContain("Waiting");
  const [request] = questions.pending();
  await questions.answer(request!.id, [{ option: CHOICE.session }]);
  advance(QUESTION_TTL);
  const edit = { path: "link", edits: [{ oldText: "first", newText: "next" }] };
  expect(approvals.write("edit", edit, outside)).toBeUndefined();
  expect(approvals.write("write", { path: "link", content: "first" },
    { ...outside, canonical: "/outside/sibling.txt" })).toContain("destination changed");
  expect(approvals.write("edit", edit, { action: "deny", reason: "protected" })).toBe("protected");
  approvals.clear();
  questions.restore();
  expect(approvals.write("edit", edit, outside)).toContain("Waiting");
});

test("a command with a supplementary format character is shown losslessly", () => {
  const { questions, approvals } = fixture();
  const command = "echo \u{e0001}";
  approvals.shell("bash", { command });
  const prompt = cardDetail(questionEntries(questions)[0]!.card).questions[0]!.text;
  expect(JSON.parse(prompt.split("\n").at(-1)!)).toBe(command);
});

test("approval is not offered if the app would truncate a command", () => {
  const { questions, approvals } = fixture();
  expect(approvals.shell("bash", { command: `echo '${"✨".repeat(350)}'` })).toContain("does not fit");
  expect(questions.pending()).toHaveLength(0);
});
