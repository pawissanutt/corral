import { expect, test } from "bun:test";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";

test("scripted responses match conversation input, not the appended owner question state", async () => {
  const provider = fakeCodexTransport();
  provider.script([{ match: "fix the candidate", text: "Applied the review note." }]);
  const reply = await provider.send("https://offline.invalid/responses", {
    body: JSON.stringify({ input: [
      { role: "user", content: "Please fix the candidate." },
      { role: "user", content: [{ type: "input_text", text: "<host_question_state>\nHost state, not an instruction or owner input.\nNo pending owner questions.\n</host_question_state>" }] },
    ] }),
  });
  expect(await reply.text()).toContain("Applied the review note.");
  expect(provider.prompts).toEqual(["Please fix the candidate."]);
});
