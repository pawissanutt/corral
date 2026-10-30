import { expect, test } from "bun:test";
import { spawn } from "node:child_process";
import { appendFileSync, linkSync, mkdirSync, mkdtempSync, readSync, renameSync, rmSync, truncateSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventFollower, ok } from "./workspace.ts";

test("a follower observes writes to its inode through a path outside the watched directory", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-follower-"));
  const watched = join(root, "watched");
  mkdirSync(watched);
  const path = join(watched, "events.jsonl");
  const writer = join(root, "writer.jsonl");
  writeFileSync(path, "");
  linkSync(path, writer);
  const follower = new EventFollower(path);
  try {
    // Given a subscription before an append that produces no watched-directory notification:
    const event = { kind: "agent_bound", detail: { generation: 2 } };
    const bound = follower.next((entry) => entry.kind === "agent_bound", 1000);
    // When a writer appends through another path to the same inode:
    appendFileSync(writer, `${JSON.stringify(event)}\n`);
    // Then the committed event resolves without another next call or a directory mutation:
    expect(await bound).toEqual(event);

    // When the file is replaced, the directory subscription reconnects to the new inode:
    const replacement = join(root, "replacement.jsonl");
    const newWriter = join(root, "new-writer.jsonl");
    writeFileSync(replacement, '{"kind":"replacement"}\n');
    linkSync(replacement, newWriter);
    const replaced = follower.next((entry) => entry.kind === "replacement", 1000);
    renameSync(replacement, path);
    expect(await replaced).toEqual({ kind: "replacement" });
    const appended = follower.next((entry) => entry.kind === "new_inode", 1000);
    appendFileSync(newWriter, '{"kind":"new_inode"}\n');
    expect(await appended).toEqual({ kind: "new_inode" });
  } finally {
    follower.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test("a follower resolves filtered events appended by another process", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-follower-"));
  const path = join(root, "events.jsonl");
  writeFileSync(path, "");
  const follower = new EventFollower(path);
  try {
    // Given a registered waiter, when a separate process appends a matching event:
    const ended = follower.next((event) => event.kind === "agent_bound" && event.detail?.turn === 1);
    const event = { kind: "agent_bound", detail: { turn: 1 } };
    const child = spawn(process.execPath, ["-e",
      'require("node:fs").appendFileSync(process.argv[1], process.argv[2])', path, `${JSON.stringify(event)}\n`],
    { stdio: ["ignore", "ignore", "inherit"] });
    const exited = new Promise<number | null>((resolve, reject) => {
      child.once("error", reject);
      child.once("exit", resolve);
    });
    expect(await exited).toBe(0);
    // Then the event resolves through the file notification without another next call:
    expect(await ended).toEqual(event);
  } finally {
    follower.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test("a follower observes a child commit while the parent waits synchronously", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-follower-"));
  const path = join(root, "events.jsonl");
  writeFileSync(path, "");
  const follower = new EventFollower(path);
  try {
    const event = { kind: "agent_bound", detail: { generation: 2 } };
    const bound = follower.next((entry) =>
      entry.kind === "agent_bound" && entry.detail?.generation === 2, 1000);
    const committed = ok([process.execPath, "-e",
      'require("node:fs").appendFileSync(process.argv[1], process.argv[2]); process.stdout.write("COMMITTED\\n")',
      path, `${JSON.stringify(event)}\n`]);
    expect(committed).toBe("COMMITTED\n");
    expect(await bound).toEqual(event);
    expect(follower.malformed).toBe(0);
  } finally {
    follower.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test("a follower observes each commit a writer makes as soon as the previous wait resolves", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-follower-"));
  const path = join(root, "events.jsonl");
  writeFileSync(path, "");
  const follower = new EventFollower(path);
  // A long-lived writer process commits one generation's agent_bound per line it reads.
  const writer = spawn(process.execPath, ["-e", `
    const fs = require("node:fs");
    require("node:readline").createInterface({ input: process.stdin }).on("line", (generation) =>
      fs.appendFileSync(process.argv[1], JSON.stringify({ kind: "agent_bound", detail: { generation: Number(generation) } }) + "\\n"));`,
  path], { stdio: ["pipe", "ignore", "inherit"] });
  const exited = new Promise<number | null>((resolve, reject) => {
    writer.once("error", reject);
    writer.once("exit", resolve);
  });
  try {
    for (let generation = 1; generation <= 6; generation++) {
      // Given a waiter, when the writer commits in the continuation of the previous resolution and
      // nothing else writes, then that commit alone resolves it. Bun's fs.watch dropped such commits.
      const bound = follower.next((entry) =>
        entry.kind === "agent_bound" && entry.detail?.generation === generation, 1000, `generation ${generation}`);
      writer.stdin?.write(`${generation}\n`);
      expect(await bound).toEqual({ kind: "agent_bound", detail: { generation } });
    }
    expect(follower.malformed).toBe(0);
  } finally {
    writer.stdin?.end();
    await exited;
    follower.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test("a follower drains short reads without requiring another file write", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-follower-"));
  const path = join(root, "events.jsonl");
  writeFileSync(path, "");
  let reads = 0;
  const follower = new EventFollower(path, (fd, buffer, offset, length, position) => {
    reads++;
    return readSync(fd, buffer, offset, Math.min(length, 7), position);
  });
  try {
    const event = { kind: "agent_bound", detail: { generation: 2 } };
    const bound = follower.next((entry) =>
      entry.kind === "agent_bound" && entry.detail?.generation === 2, 1000);
    appendFileSync(path, `${JSON.stringify(event)}\n`);
    expect(await bound).toEqual(event);
    expect(reads).toBeGreaterThan(1);
    expect(follower.malformed).toBe(0);
  } finally {
    follower.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test("a follower retains unread bytes when the file truncates between stat and read", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-follower-"));
  const path = join(root, "events.jsonl");
  writeFileSync(path, "");
  let truncated = false;
  const follower = new EventFollower(path, (fd, buffer, offset, length, position) => {
    if (!truncated) {
      truncated = true;
      truncateSync(path, 0);
    }
    return readSync(fd, buffer, offset, length, position);
  });
  try {
    appendFileSync(path, '{"kind":"other"}\n');
    const event = { kind: "agent_bound", detail: { generation: 2 } };
    // The synchronous drain sees the old stat size, but its actual read reaches EOF.
    const bound = follower.next((entry) =>
      entry.kind === "agent_bound" && entry.detail?.generation === 2, 1000);
    // Regrow before yielding: no intermediate notification can repair a skipped prefix.
    appendFileSync(path, `${JSON.stringify(event)}\n`);
    expect(await bound).toEqual(event);
    expect(truncated).toBeTrue();
    expect(follower.malformed).toBe(0);
  } finally {
    follower.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test("a follower preserves partial lines, filters and replacement files", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-follower-"));
  const path = join(root, "events.jsonl");
  writeFileSync(path, '{"kind":"historical"}\n');
  const follower = new EventFollower(path);
  try {
    // Given a partial matching record preceded by an unrelated event:
    const event = { kind: "wanted", detail: { text: "fragment" } };
    const raw = JSON.stringify(event);
    appendFileSync(path, `{"kind":"other"}\n${raw.slice(0, 10)}`);
    const first = follower.next((entry) => entry.kind === "wanted");
    // When the rest commits, then only the complete matching record resolves:
    appendFileSync(path, `${raw.slice(10)}\n`);
    expect(await first).toEqual(event);
    expect(follower.malformed).toBe(0);

    // Given a waiter before atomic replacement, when its new file commits:
    const second = follower.next((entry) => entry.kind === "replacement");
    const replacement = join(root, "replacement");
    writeFileSync(replacement, '{"kind":"replacement"}\n');
    renameSync(replacement, path);
    // Then the new inode is followed from its beginning:
    expect(await second).toEqual({ kind: "replacement" });
    expect(follower.malformed).toBe(0);
  } finally {
    follower.close();
    rmSync(root, { recursive: true, force: true });
  }
});
