import { expect, test } from "bun:test";
import { Type } from "typebox";
import { z } from "zod";

import { jsonSchemaToZodShape } from "../src/claude/custom-tools.ts";
import { sessionTodoTool, todoTool } from "../src/roles/common/todo.ts";

test("a root union of objects reaches Claude as one object with every variant's keys", () => {
  const shape = z.object(jsonSchemaToZodShape(sessionTodoTool.parameters));
  const schema = z.toJSONSchema(shape) as { properties: Record<string, unknown>; required?: string[] };
  expect(Object.keys(schema.properties).sort())
    .toEqual(["deadline", "label", "on", "op", "phase", "phases", "plan", "reason", "step", "steps", "when"]);
  // Only `op` is required by every variant.
  expect(schema.required).toEqual(["op"]);
  // Calls the model makes for different ops all fit the merged shape.
  expect(shape.safeParse({ op: "init", phases: [{ id: "a", label: "A", steps: [{ id: "s", label: "S" }] }] }).success)
    .toBe(true);
  expect(shape.safeParse({ op: "start", plan: "P2", step: "s" }).success).toBe(true);
  expect(shape.safeParse({ op: "wait", step: "s", on: { kind: "owner", question: "q?" } }).success).toBe(true);
  expect(shape.safeParse({ op: "rename", step: "s" }).success).toBe(false);
  expect(shape.safeParse({ step: "s" }).success).toBe(false);
});

test("a key shared by variants with different schemas accepts each of them", () => {
  const shape = z.object(jsonSchemaToZodShape(todoTool.parameters));
  // `steps` is a step array in init's phases and append; `step` is a string id elsewhere.
  expect(shape.safeParse({ op: "append", phase: "p", steps: [{ id: "s", label: "S", criteria: ["c"] }] }).success)
    .toBe(true);
  expect(shape.safeParse({ op: "drop", step: "s", reason: "gone" }).success).toBe(true);
  const mixed = z.object(jsonSchemaToZodShape(Type.Union([
    Type.Object({ kind: Type.Literal("a"), value: Type.String() }),
    Type.Object({ kind: Type.Literal("b"), value: Type.Number() }),
  ])));
  expect(mixed.safeParse({ kind: "a", value: "x" }).success).toBe(true);
  expect(mixed.safeParse({ kind: "b", value: 1 }).success).toBe(true);
  expect(mixed.safeParse({ kind: "b", value: true }).success).toBe(false);
});

test("plain object schemas are unchanged", () => {
  const shape = z.object(jsonSchemaToZodShape(Type.Object({ id: Type.String(), json: Type.Optional(Type.Boolean()) })));
  const schema = z.toJSONSchema(shape) as { properties: Record<string, unknown>; required?: string[] };
  expect(Object.keys(schema.properties)).toEqual(["id", "json"]);
  expect(schema.required).toEqual(["id"]);
});
