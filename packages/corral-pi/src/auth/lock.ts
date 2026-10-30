// Cross-process exclusion for the credential file: an exclusive flock(2) on a persistent lock
// file. The kernel drops it when its holder exits or dies, so there is no stale-owner recovery,
// the lock file is never unlinked, and no holder can remove another holder's lock.
import { closeSync, constants, fstatSync, openSync } from "node:fs";

import { FFIType, dlopen, read } from "bun:ffi";

import { CredentialsUnavailable } from "./paths.ts";

const LOCK_EX = 2;
const LOCK_NB = 4;
const LOCK_UN = 8;
const WOULD_BLOCK = new Set([11, 35]);
const WAIT_MS = 30_000;
const BACKOFF_MS = 25;

interface Libc {
  flock(fd: number, operation: number): number;
  errno(): number;
}

let libc: Libc | undefined;

function load(): Libc {
  if (libc) return libc;
  const flock = { args: [FFIType.i32, FFIType.i32], returns: FFIType.i32 };
  const errno = { args: [], returns: FFIType.ptr };
  if (process.platform === "darwin") {
    const { symbols } = dlopen("libSystem.B.dylib", { flock, __error: errno });
    libc = { flock: (fd, op) => symbols.flock(fd, op) as number, errno: () => read.i32(symbols.__error() as number) };
  } else {
    const { symbols } = dlopen("libc.so.6", { flock, __errno_location: errno });
    libc = {
      flock: (fd, op) => symbols.flock(fd, op) as number,
      errno: () => read.i32(symbols.__errno_location() as number),
    };
  }
  return libc;
}

export interface LockHooks {
  /** Called once when another holder has the lock and this caller starts waiting. */
  readonly onContended?: () => void;
}

export async function withFileLock<T>(path: string, work: () => Promise<T>, hooks: LockHooks = {}): Promise<T> {
  const fd = openSync(path, constants.O_RDWR | constants.O_CREAT | constants.O_NOFOLLOW, 0o600);
  try {
    if (!fstatSync(fd).isFile()) throw new CredentialsUnavailable(`${path} must be a regular file`);
    const { flock, errno } = load();
    const deadline = Date.now() + WAIT_MS;
    let contended = false;
    while (flock(fd, LOCK_EX | LOCK_NB) !== 0) {
      const code = errno();
      if (!WOULD_BLOCK.has(code)) throw new CredentialsUnavailable(`flock ${path} failed (errno ${code})`);
      if (!contended) {
        contended = true;
        hooks.onContended?.();
      }
      if (Date.now() > deadline) throw new CredentialsUnavailable(`${path} stayed locked for ${WAIT_MS} ms`);
      await new Promise((resolve) => setTimeout(resolve, BACKOFF_MS));
    }
    try {
      return await work();
    } finally {
      flock(fd, LOCK_UN);
    }
  } finally {
    closeSync(fd);
  }
}
