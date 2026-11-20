// The owner's favorite models of a Corral workspace, kept in `.corral-next/model-favorites.json`. Every session's host
// reads and writes the same file, so the terminal UI's picker and the app's (through any session's host) share them,
// and they outlive restarts. A model is its provider and its id together: the same id from two providers is two models.
import { closeSync, fstatSync, openSync, readSync, renameSync, statSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";

export interface ModelRef { readonly provider: string; readonly id: string }

/** What the pickers need: whether a model is a favorite, and marking or unmarking one. */
export interface FavoriteModels {
  has(model: ModelRef): boolean;
  /** Throws when the file cannot be written; the favorites are then unchanged. */
  set(model: ModelRef, favorite: boolean): void;
}

/** A file larger than this, or with more favorites, is not the owner's list; it reads as none rather than exhausting memory. */
const FILE_MAX_BYTES = 256 * 1024;
const FAVORITES_MAX = 1000;
const PART_MAX_BYTES = 512;

const key = (model: ModelRef) => JSON.stringify([model.provider, model.id]);

function valid(model: unknown): model is ModelRef {
  if (!model || typeof model !== "object") return false;
  const { provider, id } = model as Record<string, unknown>;
  return typeof provider === "string" && typeof id === "string" && provider.length > 0 && id.length > 0
    && Buffer.byteLength(provider) <= PART_MAX_BYTES && Buffer.byteLength(id) <= PART_MAX_BYTES;
}

/** Reads a regular file of at most FILE_MAX_BYTES; anything else reads as no favorites. */
function readBounded(path: string): string | undefined {
  let fd: number;
  try { fd = openSync(path, "r"); } catch { return undefined; }
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.size > FILE_MAX_BYTES) return undefined;
    const buffer = Buffer.alloc(stat.size);
    let read = 0;
    while (read < stat.size) {
      const n = readSync(fd, buffer, read, stat.size - read, read);
      if (n === 0) break;
      read += n;
    }
    return buffer.subarray(0, read).toString("utf8");
  } catch { return undefined; } finally { closeSync(fd); }
}

function parse(text: string | undefined): ModelRef[] {
  if (text === undefined) return [];
  try {
    const value = JSON.parse(text) as { favorites?: unknown };
    return Array.isArray(value.favorites) ? value.favorites.filter(valid).slice(0, FAVORITES_MAX)
      .map(({ provider, id }) => ({ provider, id })) : [];
  } catch { return []; }
}

export class WorkspaceModelFavorites implements FavoriteModels {
  readonly path: string;
  private cache: { readonly stamp: string; readonly models: readonly ModelRef[]; readonly keys: ReadonlySet<string> } | undefined;

  /** `changed` runs after this process changes the favorites. */
  constructor(root: string, private readonly changed?: () => void) {
    this.path = join(root, ".corral-next", "model-favorites.json");
  }

  /** The favorites now, reread only when the file changed (another session may have written it). */
  private read(): { readonly models: readonly ModelRef[]; readonly keys: ReadonlySet<string> } {
    const stat = statSync(this.path, { throwIfNoEntry: false });
    const stamp = stat ? `${stat.ino}:${stat.mtimeMs}:${stat.size}` : "";
    if (this.cache?.stamp === stamp) return this.cache;
    const models = stamp ? parse(readBounded(this.path)) : [];
    this.cache = { stamp, models, keys: new Set(models.map(key)) };
    return this.cache;
  }

  has(model: ModelRef): boolean { return this.read().keys.has(key(model)); }

  set(model: ModelRef, favorite: boolean): void {
    if (!valid(model)) throw new Error("a favorite needs a provider and a model id");
    const { models, keys } = this.read();
    if (keys.has(key(model)) === favorite) return;
    if (favorite && models.length >= FAVORITES_MAX) throw new Error(`at most ${FAVORITES_MAX} models can be favorites`);
    const next = favorite ? [...models, { provider: model.provider, id: model.id }]
      : models.filter(item => key(item) !== key(model));
    mkdirSync(dirname(this.path), { recursive: true });
    const temporary = `${this.path}.${process.pid}.tmp`;
    writeFileSync(temporary, `${JSON.stringify({ version: 1, favorites: next }, null, 2)}\n`, { mode: 0o600 });
    renameSync(temporary, this.path);
    this.cache = undefined;
    this.changed?.();
  }
}
