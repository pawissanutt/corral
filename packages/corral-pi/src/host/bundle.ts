// The modules a compiled host carries inside its own executable (src/host/compiled.ts) instead of
// loading them from an installation's src/ tree. A Bun-run host never sets it, so the fixed-path
// loader stays the only path for source and fixture installations. Bare mode (`corral pi --bare`)
// bundles only auth, so no role or delivery module is ever loaded or initialized there.
import type { Role } from "../roles/types.ts";

export interface Bundle {
  readonly roles?: Readonly<Record<Role, (root: string) => unknown>>;
  readonly delivery?: unknown;
  readonly auth: unknown;
}

let bundle: Bundle | undefined;

export function setBundle(value: Bundle | undefined): void {
  bundle = value;
}

export function compiledBundle(): Bundle | undefined {
  return bundle;
}
