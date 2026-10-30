// The connecting process's credentials on a Unix socket (Linux SO_PEERCRED through bun:ffi).
// Unknown credentials are refused by the bridge, never assumed to be the owner.
import type { Socket } from "node:net";

export interface PeerCredentials {
  readonly pid: number;
  readonly uid: number;
  readonly gid: number;
}

export type PeerLookup = (socket: Socket) => PeerCredentials | undefined;

const SOL_SOCKET = 1;
const SO_PEERCRED = 17;

interface Ffi {
  dlopen(path: string, symbols: Record<string, { args: string[]; returns: string }>): {
    symbols: { getsockopt(fd: number, level: number, name: number, value: unknown, length: unknown): number };
  };
  ptr(view: ArrayBufferView): unknown;
}

let ffi: Ffi | undefined;

export async function loadPeerLookup(): Promise<PeerLookup> {
  if (process.platform !== "linux") return () => undefined;
  ffi ??= (await import("bun:ffi" as string)) as Ffi;
  const loaded = ffi;
  const libc = loaded.dlopen("libc.so.6", {
    getsockopt: { args: ["i32", "i32", "i32", "ptr", "ptr"], returns: "i32" },
  });
  return (socket) => {
    const fd = (socket as unknown as { _handle?: { fd?: unknown } })._handle?.fd;
    if (typeof fd !== "number" || fd < 0) return undefined;
    const ucred = new Int32Array(3);
    const length = new Uint32Array([ucred.byteLength]);
    const status = libc.symbols.getsockopt(fd, SOL_SOCKET, SO_PEERCRED, loaded.ptr(ucred), loaded.ptr(length));
    const [pid, uid, gid] = ucred;
    if (status !== 0 || pid === undefined || uid === undefined || gid === undefined) return undefined;
    return { pid, uid, gid };
  };
}
