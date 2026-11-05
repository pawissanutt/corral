import { expect, test } from "bun:test";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../bridge/events.ts";
import { CorralClient, type CorralCall } from "../tools/corral.ts";
import { recordQuestionOwnerInput } from "./question-owner-input.ts";

test("a lost owner-input receipt reuses its persisted event identity and original turn", async () => {
  const calls: CorralCall[] = [];
  class Client extends CorralClient {
    override async call(_op: "event", call: CorralCall) {
      calls.push(call);
      if (calls.length === 1) throw new Error("receipt lost after commit");
      return { version: 1, request_id: call.requestId, op: "event", state: "committed" as const,
        result: { input: "A-1/g1/i1" }, replayed: true };
    }
  }
  const client = new Client({ corral: "unused", root: "/unused", identity: { store: "st-demo", project: "", role: "assistant", generation: 1 },
    hostCap: "unused", sessionId: () => "owned" });
  const options = { client, manager: SessionManager.inMemory(), events: new EventLog(1), generation: 1,
    turn: 1, question: "Q1", text: "[ask_user answer Q1]\nWhich?: Small" };
  await expect(recordQuestionOwnerInput(options)).rejects.toThrow("receipt lost");
  options.events.emit("unrelated");
  const text = await recordQuestionOwnerInput({ ...options, turn: 2 });
  expect(calls).toHaveLength(2);
  expect(calls[1]).toEqual(calls[0]);
  expect(text).toBe(`${options.text}\n\n[owner input A-1/g1/i1]`);
});
