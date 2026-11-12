import { expect, test } from "bun:test";
import { notesTool } from "./notes.ts";

test("PM notes tool sends the typed notes payload", () => {
  const payload = { op: "add", kind: "instruction", text: "Keep context", provenance: "message-1", expected_revision: 2 };
  expect(notesTool.op).toBe("notes");
  expect(notesTool.payload(payload)).toEqual(payload);
});
