import { expect, test } from "bun:test";
import { existsSync, linkSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { tryWorktreeLock, WorktreeFiles } from "./worktree-files.ts";

const unixTest = test.skipIf(process.platform !== "linux" && process.platform !== "darwin");

function fixture(run: (base: string, checkout: string, files: WorktreeFiles) => void): void {
  const base = mkdtempSync(join(realpathSync.native(tmpdir()), "corral-worktree-files-"));
  const checkout = join(base, ".worktrees", "T-1");
  try {
    mkdirSync(checkout, { recursive: true });
    const { dev, ino } = lstatSync(checkout, { bigint: true });
    const files = new WorktreeFiles(checkout, String(dev), String(ino));
    try { run(base, checkout, files); }
    finally { files.close(); }
  } finally { rmSync(base, { recursive: true, force: true }); }
}

unixTest("scoped operations create and edit source files without allowing edit to create", () => {
  fixture((_base, checkout, files) => {
    const sourceDir = join(checkout, "src", "nested");
    const source = join(sourceDir, "example.txt");
    files.mkdir(checkout); // SDK write at the checkout root mkdirs the existing root.
    files.mkdir(sourceDir);
    files.mkdir(sourceDir);
    expect(() => files.write(source, "missing", false)).toThrow();
    expect(existsSync(source)).toBeFalse();
    files.write(source, "first version\n", true);
    const ordinary = join(sourceDir, "ordinary.txt");
    writeFileSync(ordinary, "");
    expect(lstatSync(source).mode & 0o777).toBe(lstatSync(ordinary).mode & 0o777);
    files.access(source);
    expect(files.read(source)).toEqual(Buffer.from("first version\n"));
    files.write(source, "é\n", false);
    expect(readFileSync(source, "utf8")).toBe("é\n");
    files.write(source, "", true);
    expect(readFileSync(source)).toHaveLength(0);
  });
});

unixTest("source operations reject protected paths, checkout escapes, and lossy names", () => {
  fixture((base, checkout, files) => {
    for (const protectedName of [".GiT", ".CORRAL", ".Corral-Next", ".WorkTrees"]) {
      const protectedDir = join(checkout, protectedName);
      expect(() => files.mkdir(protectedDir)).toThrow();
      mkdirSync(protectedDir);
      expect(() => files.write(join(protectedDir, "private"), "wrong", true)).toThrow();
    }
    const other = join(base, ".worktrees", "T-10");
    mkdirSync(other);
    const invalid = [join(other, "outside"), `${checkout}/../sibling`, `${checkout}/nul\0ignored`, `${checkout}/\uD800`];
    for (const path of invalid) expect(() => files.write(path, "wrong", true)).toThrow();
    expect(existsSync(join(checkout, "�"))).toBeFalse();
    expect(existsSync(join(other, "outside"))).toBeFalse();
    const nested = join(checkout, ".Corral", ".worktrees", "T-2");
    mkdirSync(nested, { recursive: true });
    const { dev, ino } = lstatSync(nested, { bigint: true });
    expect(() => new WorktreeFiles(nested, String(dev), String(ino))).toThrow();
  });
});

unixTest("descriptor-relative traversal refuses symlinks, hardlinks and directories before truncating", () => {
  fixture((base, checkout, files) => {
    const state = join(base, ".corral-next");
    const secret = join(state, "private");
    mkdirSync(state);
    writeFileSync(secret, "protected");
    const hardlink = join(checkout, "hardlink");
    linkSync(secret, hardlink);
    const symlink = join(checkout, "symlink");
    symlinkSync(secret, symlink);
    const alias = join(checkout, "alias");
    symlinkSync(state, alias, "dir");
    for (const target of [hardlink, symlink, join(alias, "private")]) {
      expect(() => files.access(target)).toThrow();
      expect(() => files.read(target)).toThrow();
      expect(() => files.write(target, "wrong", true)).toThrow();
      expect(() => files.write(target, "wrong", false)).toThrow();
    }
    expect(() => files.mkdir(join(alias, "escaped"))).toThrow();
    expect(existsSync(join(state, "escaped"))).toBeFalse();
    expect(readFileSync(secret, "utf8")).toBe("protected");
    const dir = join(checkout, "dir");
    files.mkdir(dir);
    expect(() => files.read(dir)).toThrow();
  });
});

unixTest("a moved checkout cannot be reused as a replacement directory or symlink", () => {
  fixture((_base, checkout, files) => {
    const source = join(checkout, "source");
    files.write(source, "original", true);
    const old = `${checkout}-old`;
    renameSync(checkout, old);
    mkdirSync(checkout);
    writeFileSync(source, "replacement");
    expect(() => files.check()).toThrow();
    expect(() => files.write(source, "wrong", false)).toThrow();
    expect(readFileSync(source, "utf8")).toBe("replacement");
    rmSync(checkout, { recursive: true });
    symlinkSync(old, checkout, "dir");
    expect(() => files.write(source, "wrong", false)).toThrow();
    expect(readFileSync(join(old, "source"), "utf8")).toBe("original");
  });
});

unixTest("persistent locks share, exclude and release through the kernel", () => {
  fixture((base) => {
    const path = join(base, "task.lock");
    expect(() => tryWorktreeLock(path, true)).toThrow();
    expect(existsSync(path)).toBeFalse();
    writeFileSync(path, "");
    const first = tryWorktreeLock(path, false);
    try {
      const second = tryWorktreeLock(path, false);
      try {
        first.check();
        second.check();
        expect(() => tryWorktreeLock(path, true)).toThrow();
      } finally { second.close(); }
      expect(() => tryWorktreeLock(path, true)).toThrow();
    } finally { first.close(); }
    const exclusive = tryWorktreeLock(path, true);
    try {
      const { dev, ino } = lstatSync(path, { bigint: true });
      expect(exclusive.device).toBe(String(dev));
      expect(exclusive.inode).toBe(String(ino));
      expect(() => tryWorktreeLock(path, false)).toThrow();
      expect(() => tryWorktreeLock(path, true)).toThrow();
    } finally { exclusive.close(); }
    expect(() => exclusive.check()).toThrow();
    tryWorktreeLock(path, true).close();
  });
});

unixTest("lock checks fail closed when the persistent entry becomes aliased or is replaced", () => {
  fixture((base) => {
    const path = join(base, "task.lock");
    const alias = join(base, "alias.lock");
    writeFileSync(path, "");
    const lock = tryWorktreeLock(path, true);
    try {
      linkSync(path, alias);
      expect(() => lock.check()).toThrow();
      expect(() => tryWorktreeLock(alias, false)).toThrow();
      rmSync(alias);
      lock.check();
      renameSync(path, alias);
      symlinkSync(alias, path);
      expect(() => lock.check()).toThrow();
      expect(() => tryWorktreeLock(path, false)).toThrow();
      rmSync(path);
      writeFileSync(path, "");
      expect(() => lock.check()).toThrow();
    } finally { lock.close(); }
  });
});
