import { expect, test } from "bun:test";
import { appendFileSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { writeFakeCorral } from "./support/fake-corral.ts";

test("fake corral reads only complete log records", () => {
  const dir = mkdtempSync(join(tmpdir(), "fake-corral-log-"));
  try {
    const fake = writeFakeCorral(dir);
    const log = join(dir, "calls.jsonl");
    const record = JSON.stringify({ argv: [], stdin: JSON.stringify({ op: "event" }), envKeys: [] });
    writeFileSync(log, record + "\n" + record.slice(0, 10));
    expect(fake.calls().map((call) => call.envelope.op)).toEqual(["event"]);
    appendFileSync(log, record.slice(10) + "\n");
    expect(fake.calls().map((call) => call.envelope.op)).toEqual(["event", "event"]);
    writeFileSync(log, "broken\n");
    expect(() => fake.calls()).toThrow();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
