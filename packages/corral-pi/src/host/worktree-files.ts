import { closeSync, constants as f, fstatSync, ftruncateSync, lstatSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import type { BigIntStats } from "node:fs";
import { dlopen, FFIType, ptr, read } from "bun:ffi";

const PROTECTED = new Set([".git", ".corral", ".corral-next", ".worktrees"]);
const DIRECTORY = f.O_RDONLY | f.O_DIRECTORY;
const NOT_FOLLOWED = f.O_NOFOLLOW | f.O_NONBLOCK | f.O_NOCTTY;
const LOCK_SH = 1, LOCK_EX = 2, LOCK_NB = 4;
const ENOENT = 2, EEXIST = 17;

function loadFileOps() {
  const apple = process.platform === "darwin";
  if (!apple && process.platform !== "linux") throw new Error("Secure worktree access is unsupported on this platform.");
  // Darwin's openat is variadic: an arm64 vararg is passed on the stack, not
  // in the register used by bun:ffi for a fixed fourth argument. Use the
  // fixed-arity syscall wrapper that Darwin's openat itself delegates to.
  const openat = apple ? "__openat" : "openat";
  const errno = apple ? "__error" : "__errno_location";
  const { symbols } = dlopen(apple ? "libSystem.B.dylib" : "libc.so.6", {
    [openat]: { args: [FFIType.i32, FFIType.ptr, FFIType.i32, FFIType.i32], returns: FFIType.i32 },
    mkdirat: { args: [FFIType.i32, FFIType.ptr, FFIType.i32], returns: FFIType.i32 },
    flock: { args: [FFIType.i32, FFIType.i32], returns: FFIType.i32 },
    [errno]: { args: [], returns: FFIType.ptr },
  });
  return {
    openat: symbols[openat]!, mkdirat: symbols.mkdirat, flock: symbols.flock,
    errno: () => read.i32(symbols[errno]!() as number),
    cloexec: apple ? 0x01000000 : 0o2000000,
    cwd: apple ? -2 : -100, // AT_FDCWD (only used with absolute paths)
    wouldBlock: apple ? 35 : 11,
  };
}
let fileOps: ReturnType<typeof loadFileOps> | undefined;
const native = () => fileOps ??= loadFileOps();

class OpenError extends Error {
  constructor(readonly errno: number) { super(`Could not open worktree entry (errno ${errno}).`); }
}

function openAt(parent: number, name: string, flags: number, mode = 0): number {
  const ops = native();
  // All callers validate the pathname before passing it or its components here.
  const bytes = Buffer.from(`${name}\0`);
  const fd = ops.openat(parent, ptr(bytes), flags | NOT_FOLLOWED | ops.cloexec, mode) as number;
  if (fd < 0) throw new OpenError(ops.errno());
  return fd;
}

function openDirectory(parent: number, name: string, create: boolean): number {
  try { return openAt(parent, name, DIRECTORY); }
  catch (error) {
    if (!create || !(error instanceof OpenError) || error.errno !== ENOENT) throw error;
    const ops = native();
    const bytes = Buffer.from(`${name}\0`);
    if (ops.mkdirat(parent, ptr(bytes), 0o777) !== 0) {
      const errno = ops.errno();
      if (errno !== EEXIST) throw new Error(`Could not create worktree directory (errno ${errno}).`);
    }
    // Whether we created it or it already exists, don't follow a symlink.
    return openAt(parent, name, DIRECTORY);
  }
}

function pathParts(path: string): string[] {
  const parts = path.slice(1).split("/");
  if (!path.startsWith("/") || path.includes("\0") || Buffer.from(path).toString("utf8") !== path
    || parts.some(part => !part || part === "." || part === "..")) {
    throw new Error("Expected a valid absolute worktree path without traversal components.");
  }
  return parts;
}

function canonicalEntry(path: string): BigIntStats {
  const bytes = realpathSync.native(path, { encoding: "buffer" });
  if (!bytes.equals(Buffer.from(path))) throw new Error("Worktree path is not canonical.");
  return lstatSync(path, { bigint: true });
}

const stat = (fd: number) => fstatSync(fd, { bigint: true });
const same = (a: BigIntStats, b: BigIntStats) => a.dev === b.dev && a.ino === b.ino;
const regular = (entry: BigIntStats) => entry.isFile() && entry.nlink === 1n;

/** Filesystem operations for the one checkout pinned during owner approval. */
export class WorktreeFiles {
  #fd: number | undefined;
  readonly #checkout: string;
  readonly #identity: BigIntStats;

  constructor(checkout: string, device: string, inode: string) {
    const parts = pathParts(checkout);
    if (parts.some((part, i) => PROTECTED.has(part.toLowerCase())
      && !(i === parts.length - 2 && part.toLowerCase() === ".worktrees"))) {
      throw new Error("Checkout is inside protected Git or Corral state.");
    }
    this.#checkout = checkout;
    const fd = openAt(native().cwd, checkout, DIRECTORY);
    try {
      this.#identity = stat(fd);
      if (this.#identity.dev.toString() !== device || this.#identity.ino.toString() !== inode) {
        throw new Error("Checkout identity changed since task lookup.");
      }
      this.#fd = fd;
      this.check();
    } catch (error) {
      closeSync(fd);
      throw error;
    }
  }

  close(): void {
    const fd = this.#fd;
    this.#fd = undefined;
    if (fd !== undefined) closeSync(fd);
  }

  check(): void { this.#checkedRoot(); }

  #checkedRoot(): number {
    const fd = this.#fd;
    if (fd === undefined) throw new Error("Worktree access is closed.");
    const named = canonicalEntry(this.#checkout);
    if (!named.isDirectory() || !same(named, this.#identity) || !same(stat(fd), this.#identity)) {
      throw new Error("The approved checkout directory moved or changed identity.");
    }
    return fd;
  }

  #parts(path: string): string[] {
    if (path === this.#checkout) return [];
    if (!path.startsWith(`${this.#checkout}/`)) throw new Error("Path is outside the approved checkout.");
    const relative = pathParts(path).slice(this.#checkout.split("/").length - 1);
    if (relative.some(part => PROTECTED.has(part.toLowerCase()))) {
      throw new Error("Access to protected Git or Corral state is refused.");
    }
    return relative;
  }

  #withDirectory<T>(parts: string[], create: boolean, work: (fd: number) => T): T {
    // Use the pinned descriptor, not a re-open of the validated pathname.
    const root = this.#checkedRoot();
    let parent = root;
    try {
      for (const part of parts) {
        const previous = parent;
        parent = openDirectory(previous, part, create);
        if (previous !== root) closeSync(previous);
      }
      return work(parent);
    } finally {
      if (parent !== root) closeSync(parent);
    }
  }

  #withFile<T>(path: string, flags: number, work: (fd: number) => T): T {
    const parts = this.#parts(path);
    const name = parts.pop();
    if (!name) throw new Error("Expected a source file below the approved checkout.");
    return this.#withDirectory(parts, false, parent => {
      const fd = openAt(parent, name, flags, 0o666);
      try {
        if (!regular(stat(fd))) throw new Error("Worktree entry must be a regular, single-link file.");
        return work(fd);
      } finally { closeSync(fd); }
    });
  }

  read(path: string): Buffer {
    return this.#withFile(path, f.O_RDONLY, fd => readFileSync(fd));
  }

  access(path: string): void {
    this.#withFile(path, f.O_RDWR, () => {});
  }

  write(path: string, content: string, create: boolean): void {
    const bytes = Buffer.from(content, "utf8");
    this.#withFile(path, f.O_WRONLY | (create ? f.O_CREAT : 0), fd => {
      // O_TRUNC at open time could destroy a hard-linked file before validation.
      // Neither truncation nor the write looks up the pathname again.
      ftruncateSync(fd, 0);
      writeFileSync(fd, bytes);
    });
  }

  mkdir(path: string): void {
    this.#withDirectory(this.#parts(path), true, () => {});
  }
}

/** Lock the same persistent file as Rust File::try_lock[_shared]. Never create or unlink it. */
export function tryWorktreeLock(path: string, exclusive: boolean): {
  readonly device: string; readonly inode: string; readonly fd: number; check(): void; close(): void;
} {
  pathParts(path);
  const before = canonicalEntry(path);
  if (!regular(before)) throw new Error("Worktree lock must be a regular, single-link file.");
  const ops = native();
  const fd = openAt(ops.cwd, path, exclusive ? f.O_RDWR : f.O_RDONLY);
  let closed = false;
  const check = () => {
    if (closed) throw new Error("Worktree lock is closed.");
    const held = stat(fd);
    const named = canonicalEntry(path);
    if (!regular(held) || !regular(named) || !same(held, before) || !same(named, before)) {
      throw new Error("Worktree lock moved or changed identity.");
    }
  };
  try {
    check();
    if (ops.flock(fd, (exclusive ? LOCK_EX : LOCK_SH) | LOCK_NB) !== 0) {
      const errno = ops.errno();
      throw new Error(errno === ops.wouldBlock
        ? "Worktree lock is held by another operation."
        : `Could not lock worktree (errno ${errno}).`);
    }
    check();
    return {
      device: before.dev.toString(), inode: before.ino.toString(), fd, check,
      close() {
        if (closed) return;
        closed = true;
        closeSync(fd);
      },
    };
  } catch (error) {
    closeSync(fd);
    throw error;
  }
}
