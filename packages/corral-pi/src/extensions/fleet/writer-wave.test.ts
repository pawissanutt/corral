import { expect, test } from "bun:test";
import { validateWriterWave, type WriterWaveNode } from "./writer-wave.ts";

const writer = (id: string, owns: string[], after: string[] = []): WriterWaveNode => ({ id, kind: "slice", owns, after });
const a = writer("a", ["src/./sub/../feature/"]);
const b = writer("b", ["src/feature/file"]);

test("compares literal normalized roots on component boundaries in either order", () => {
  for (const pair of [[a, b], [b, a], [a, writer("b", ["other", "src/feature"])]]) {
    expect(() => validateWriterWave([], pair)).toThrow(/overlaps/);
  }
  expect(() => validateWriterWave([], [a, writer("b", ["src/feature-old"])])).not.toThrow();
});

test("includes all live writers but not terminal writers or read-only peers", () => {
  for (const state of ["queued", "running", "needs"] as const) {
    expect(() => validateWriterWave([{ ...a, state }], [b])).toThrow(/overlaps/);
  }
  for (const state of ["failed", "cancelled", "done"] as const) {
    expect(() => validateWriterWave([{ ...a, state }], [b])).not.toThrow();
  }
  expect(() => validateWriterWave([], [a, { ...b, kind: "review" }])).not.toThrow();
});

test("reaches transitive dependencies through read-only and existing nodes, not common prerequisites", () => {
  const scout: WriterWaveNode = { id: "scout", kind: "explore", owns: [], after: ["a"] };
  const later = { ...b, after: ["scout"] };
  expect(() => validateWriterWave([], [later, scout, a])).not.toThrow();
  expect(() => validateWriterWave([{ ...a, state: "running" }, { ...scout, state: "queued" }], [later])).not.toThrow();
  expect(() => validateWriterWave([], [{ ...scout, after: [] }, { ...a, after: ["scout"] }, later])).toThrow(/overlaps/);
});

test("unrestricted roots are only ambiguous for parallel writers", () => {
  for (const owns of [[], [""], ["."], ["x", "src/../"]]) {
    const broad = writer("b", owns);
    expect(() => validateWriterWave([], [a, broad])).toThrow(/overlaps/);
    expect(() => validateWriterWave([], [broad])).not.toThrow();
    expect(() => validateWriterWave([], [a, { ...broad, after: [a.id] }])).not.toThrow();
  }
});

test("rejects ambiguous separators, absolute paths and escaping declarations", () => {
  for (const path of ["src/../../outside", "/", "C:/outside", "src\\file", "a\0b"]) {
    expect(() => validateWriterWave([], [writer("w", [path])])).toThrow(/repository-relative/);
  }
});
