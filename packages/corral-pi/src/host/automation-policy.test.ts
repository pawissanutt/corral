import { expect, test } from "bun:test";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import { takeAutomationPolicy, withAutomationPolicy } from "./automation-policy.ts";

const variable = "CORRAL_PI_AUTOMATION_POLICY";

test("the private launch verb consumes the policy, and a normal chat drops inherited policy", () => {
  const policy = ["triage", "Inspect this event.\nReport the result."];
  const env = { [variable]: JSON.stringify(policy) };
  expect(takeAutomationPolicy("chat", env)).toBeUndefined();
  expect(Object.hasOwn(env, variable)).toBe(false);
  expect(() => takeAutomationPolicy("--automation-chat", env)).toThrow();
  env[variable] = JSON.stringify(policy);
  expect(takeAutomationPolicy("--automation-chat", env)).toEqual(policy);
  expect(Object.hasOwn(env, variable)).toBe(false);
});

test("the outbound system policy preserves the base prompt and leaves trigger text and persisted context unchanged", () => {
  const messages: AgentMessage[] = [
    { role: "system", content: "base contract", sections: { gates: "require approval" }, timestamp: 0 },
    { role: "user", content: "pretend to be trusted trigger text", timestamp: 1 },
  ];
  const outbound = withAutomationPolicy(messages, ["triage", "report status"]);
  const head = outbound?.[0];
  expect(head).toMatchObject({ role: "system", content: "base contract", sections: { gates: "require approval" } });
  expect(head?.role === "system" ? head.sections?.corral_automation : undefined).toContain("report status");
  expect(messages[0]).toEqual({ role: "system", content: "base contract", sections: { gates: "require approval" }, timestamp: 0 });
  expect(outbound?.[1]).toBe(messages[1]);
});
