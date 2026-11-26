import { expect, test } from "bun:test";
import { lstatSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { APPROVAL_CHOICES as CHOICE, isToolApproval, PendingQuestions } from "./pending-questions.ts";
import { WorktreeAccess } from "./worktree-access.ts";
import { tryWorktreeLock } from "./worktree-files.ts";

function fixture() {
  const cwd = mkdtempSync(join(realpathSync(tmpdir()), "corral-worktree-approval-"));
  const task = "T-42", checkout = join(cwd, ".worktrees", task), lock = join(cwd, "lock");
  mkdirSync(checkout, { recursive: true });
  writeFileSync(lock, "");
  const identity = (path: string) => lstatSync(path, { bigint: true });
  const result = { task, run: "R1", checkout, lock, device: String(identity(checkout).dev), inode: String(identity(checkout).ino),
    lockDevice: String(identity(lock).dev), lockInode: String(identity(lock).ino) };
  const audit: Record<string, unknown>[] = [];
  let session = "assistant-session";
  const questions = new PendingQuestions({ session: () => session,
    append: () => {}, changed: q => { if (q) access.observe(q); }, entries: () => [], deliver: async () => false });
  const access = new WorktreeAccess({ cwd, session: () => session, ready: () => true, questions,
    command: async argv => { if (argv[0] === "takeover-verify") await verification; return result; },
    audit: record => { audit.push(record); },
  });
  let verification: Promise<void> | undefined;
  const request = async () => { await access.request(task); return questions.pending()[0]!; };
  const approve = async (id: string) => { await questions.answer(id, [{ option: CHOICE.session }]); return access.request(task); };
  return { access, audit, checkout, lock, questions, request, approve, result,
    verifyAfter: (wait: Promise<void>) => { verification = wait; },
    end: () => { access.revoke("session ended"); session = "different session"; questions.close(true); },
    close: () => { access.revoke("test cleanup"); questions.close(true); rmSync(cwd, { recursive: true, force: true }); },
  };
}

test("cancelled and denied owner choices never reserve the worktree", async () => {
  const f = fixture();
  try {
    const q = await f.request();
    expect(isToolApproval(q)).toBeTrue();
    expect(f.access.decision(join(f.checkout, "x"))).toBeUndefined();
    expect(f.questions.cancel(q.id)).toBe("cancelled");
    expect(await f.questions.answer(q.id, [{ option: CHOICE.session }])).toBe("cancelled");
    const next = await f.request();
    await f.questions.answer(next.id, [{ option: CHOICE.deny }]);
    expect(await f.access.request(f.result.task)).toContain("denied");
    expect(f.access.decision(join(f.checkout, "x"))).toBeUndefined();
    expect(f.audit.map(r => r.state)).toEqual(["pending", "cancelled", "pending", "denied"]);
    tryWorktreeLock(f.lock, true).close();
  } finally { f.close(); }
});

test("owner approval can't take a writer's lock, and session exit invalidates an in-flight verification", async () => {
  const f = fixture();
  try {
    const pending = await f.request();
    // A writer that starts after the preview still wins; approving does not stop it.
    const writer = tryWorktreeLock(f.lock, false);
    try {
      await f.questions.answer(pending.id, [{ option: CHOICE.session }]);
      expect(f.audit.at(-1)?.state).toBe("unavailable");
      expect(f.access.decision(join(f.checkout, "x"))).toBeUndefined();
    } finally { writer.close(); }
    expect(await f.approve((await f.request()).id)).toContain("Granted");
    const wait = Promise.withResolvers<void>();
    f.verifyAfter(wait.promise);
    const inFlight = f.access.use(join(f.checkout, "x"), undefined, async () => { throw new Error("operation ran"); },
      async () => { throw new Error("unscoped fallback ran"); });
    await Promise.resolve();
    f.end();
    tryWorktreeLock(f.lock, true).close();
    wait.resolve();
    await expect(inFlight).rejects.toThrow("no longer current");
    expect(f.audit.at(-1)).toMatchObject({ state: "revoked", reason: "session ended" });
  } finally { f.close(); }
});
