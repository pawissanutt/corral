// The owner's favorite models of a Corral workspace, kept in `.corral-next/model-favorites.json`. Every session's host
// reads and writes the same file, so the terminal UI's picker and the app's (through any session's host) share them,
// and they outlive restarts. A model is its provider and its id together: the same id from two providers is two models.
import { closeSync, fstatSync, mkdirSync, openSync, readSync, renameSync, rmSync, statSync, watch, writeFileSync, type FSWatcher } from "node:fs";
import { basename, dirname, join } from "node:path";

export interface ModelRef { readonly provider: string; readonly id: string }

/** What the pickers need: whether a model is a favorite, and marking or unmarking one. */
export interface FavoriteModels {
  has(model: ModelRef): boolean;
  /** Throws when the file cannot be written; the favorites are then unchanged. */
  set(model: ModelRef, favorite: boolean): void;
}

const FAVORITES_MAX = 1000;
const PART_MAX_BYTES = 512;
/** Room for FAVORITES_MAX entries of the longest parts, escaped; a larger file is not one this store wrote. */
const FILE_MAX_BYTES = 8 * 1024 * 1024;
/** How long a writer waits for another host's write, and when a lock left by a crashed host is taken over. */
const LOCK_WAIT_MS = 2_000, LOCK_STALE_MS = 10_000;
/** How stale an unwatched store's view of the file may be. */
const RECHECK_MS = 1_000;

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

function stampOf(path: string): string {
  try {
    const stat = statSync(path, { throwIfNoEntry: false });
    return stat ? `${stat.ino}:${stat.mtimeMs}:${stat.size}` : "";
  } catch { return "unreadable"; }
}

const sleeper = new Int32Array(new SharedArrayBuffer(4));

/** Runs `write` holding the store's lock file, so two hosts' changes never overwrite each other. */
function locked(path: string, write: () => void): void {
  const lock = `${path}.lock`, deadline = Date.now() + LOCK_WAIT_MS;
  for (;;) {
    try { closeSync(openSync(lock, "wx", 0o600)); break; }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      const stat = statSync(lock, { throwIfNoEntry: false });
      if (stat && Date.now() - stat.mtimeMs > LOCK_STALE_MS) { rmSync(lock, { force: true }); continue; }
      if (Date.now() > deadline) throw new Error("another session is saving favorites; try again");
      Atomics.wait(sleeper, 0, 0, 10);
    }
  }
  try { write(); } finally { rmSync(lock, { force: true }); }
}

export class WorkspaceModelFavorites implements FavoriteModels {
  readonly path: string;
  private cache: { readonly stamp: string; readonly models: readonly ModelRef[]; readonly keys: ReadonlySet<string> } | undefined;
  private watcher: FSWatcher | undefined;
  private checked = 0;

  /**
   * `changed` runs after the favorites change, here or in another session's host (the file is watched while the
   * workspace's `.corral-next` directory exists), so this host re-announces its catalog to the app.
   */
  constructor(root: string, private readonly changed?: () => void) {
    this.path = join(root, ".corral-next", "model-favorites.json");
    if (changed) this.watch();
  }

  private watch(): void {
    const name = basename(this.path);
    try {
      this.watcher = watch(dirname(this.path), { persistent: false }, (_event, file) => {
        if (file !== null && file !== name) return;
        this.cache = undefined;
        this.changed?.();
      });
      this.watcher.on("error", () => { this.watcher?.close(); this.watcher = undefined; });
    } catch { this.watcher = undefined; }
  }

  close(): void { this.watcher?.close(); this.watcher = undefined; }

  /** The favorites now, reread only when the file changed (another session may have written it). */
  private read(): { readonly models: readonly ModelRef[]; readonly keys: ReadonlySet<string> } {
    // A picker asks for every row on every frame: while the file is watched the cache holds until it changes, and
    // otherwise the file is checked at most every RECHECK_MS.
    const now = Date.now();
    if (this.cache && (this.watcher || now - this.checked < RECHECK_MS)) return this.cache;
    this.checked = now;
    const stamp = stampOf(this.path);
    if (this.cache?.stamp === stamp) return this.cache;
    const models = stamp && stamp !== "unreadable" ? parse(readBounded(this.path)) : [];
    this.cache = { stamp, models, keys: new Set(models.map(key)) };
    return this.cache;
  }

  has(model: ModelRef): boolean { return this.read().keys.has(key(model)); }

  set(model: ModelRef, favorite: boolean): void {
    if (!valid(model)) throw new Error("a favorite needs a provider and a model id");
    mkdirSync(dirname(this.path), { recursive: true });
    if (this.changed && !this.watcher) this.watch();
    let wrote = false;
    locked(this.path, () => {
      this.cache = undefined;
      const { models, keys } = this.read();
      if (keys.has(key(model)) === favorite) return;
      if (favorite && models.length >= FAVORITES_MAX) throw new Error(`at most ${FAVORITES_MAX} models can be favorites`);
      const next = favorite ? [...models, { provider: model.provider, id: model.id }]
        : models.filter(item => key(item) !== key(model));
      const text = `${JSON.stringify({ version: 1, favorites: next }, null, 2)}\n`;
      if (Buffer.byteLength(text) > FILE_MAX_BYTES) throw new Error("the favorites list is too large");
      const temporary = `${this.path}.${process.pid}.tmp`;
      writeFileSync(temporary, text, { mode: 0o600 });
      renameSync(temporary, this.path);
      wrote = true;
    });
    this.cache = undefined;
    if (wrote) this.changed?.();
  }
}
