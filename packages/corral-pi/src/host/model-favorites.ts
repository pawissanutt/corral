// The owner's favorite models of a Corral workspace, kept in `.corral-next/model-favorites/`. Every session's host
// reads and writes the same directory, so the terminal UI's picker and the app's (through any session's host) share
// them, and they outlive restarts. A model is its provider and its id together: the same id from two providers is two
// models.
//
// Each favorite is its own file, named by a hash of its provider and id: marking one renames a complete file into
// place and unmarking unlinks it. Two hosts changing favorites at once therefore never read-modify-write a shared
// document, need no lock (so a crashed host leaves nothing to recover), and never lose one another's change.
import { createHash, randomBytes } from "node:crypto";
import { closeSync, fstatSync, mkdirSync, openSync, readdirSync, readSync, renameSync, rmSync, statSync, watch, writeFileSync,
  type FSWatcher } from "node:fs";
import { join } from "node:path";

export interface ModelRef { readonly provider: string; readonly id: string }

/** What the pickers need: whether a model is a favorite, and marking or unmarking one. */
export interface FavoriteModels {
  has(model: ModelRef): boolean;
  /** Throws when the change cannot be saved; the favorites are then unchanged. */
  set(model: ModelRef, favorite: boolean): void;
}

/**
 * A host refuses to add a favorite past this many. Two hosts adding at once may both pass the check, so the directory
 * can hold a few more; the reader lists every favorite file, so a save that succeeded is always visible.
 */
const FAVORITES_MAX = 1000;
const PART_MAX_BYTES = 512;
/** A favorite's file holds its two parts; anything larger is not one this store wrote. */
const FILE_MAX_BYTES = 8 * 1024;
/** How stale an unwatched store's view of the directory may be. */
const RECHECK_MS = 1_000;
const NAME = /^([0-9a-f]{40})\.json$/;

const key = (model: ModelRef) => JSON.stringify([model.provider, model.id]);
const fileName = (model: ModelRef) => `${createHash("sha256").update(key(model)).digest("hex").slice(0, 40)}.json`;

function valid(model: unknown): model is ModelRef {
  if (!model || typeof model !== "object") return false;
  const { provider, id } = model as Record<string, unknown>;
  return typeof provider === "string" && typeof id === "string" && provider.length > 0 && id.length > 0
    && Buffer.byteLength(provider) <= PART_MAX_BYTES && Buffer.byteLength(id) <= PART_MAX_BYTES;
}

/** A favorite's file: a regular file of at most FILE_MAX_BYTES naming the model its name hashes; else undefined. */
function readFavorite(dir: string, name: string): ModelRef | undefined {
  let fd: number;
  try { fd = openSync(join(dir, name), "r"); } catch { return undefined; }
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
    const value = JSON.parse(buffer.subarray(0, read).toString("utf8")) as unknown;
    if (!valid(value)) return undefined;
    const model = { provider: value.provider, id: value.id };
    return fileName(model) === name ? model : undefined;
  } catch { return undefined; } finally { closeSync(fd); }
}

export class WorkspaceModelFavorites implements FavoriteModels {
  readonly dir: string;
  private cache: { readonly stamp: string; readonly keys: ReadonlySet<string> } | undefined;
  private watcher: FSWatcher | undefined;
  private checked = 0;

  /**
   * `changed` runs after the favorites change, in this host or in another session's (the directory is watched), so
   * this host re-announces its catalog to the app and redraws an open picker.
   */
  constructor(root: string, private readonly changed?: () => void, private readonly max = FAVORITES_MAX) {
    this.dir = join(root, ".corral-next", "model-favorites");
    if (changed) this.watch();
  }

  private watch(): void {
    try {
      mkdirSync(this.dir, { recursive: true });
      this.watcher = watch(this.dir, { persistent: false }, () => {
        this.cache = undefined;
        this.changed?.();
      });
      this.watcher.on("error", () => this.close());
    } catch { this.watcher = undefined; }
  }

  close(): void { this.watcher?.close(); this.watcher = undefined; }

  private stamp(): string {
    try {
      const stat = statSync(this.dir, { throwIfNoEntry: false });
      return stat ? `${stat.ino}:${stat.mtimeMs}` : "";
    } catch { return "unreadable"; }
  }

  /**
   * The favorites now. A picker asks for every row on every frame: while the directory is watched the cache holds
   * until it changes, otherwise the directory is checked at most every RECHECK_MS. Never throws: an unreadable
   * directory or file reads as no favorite.
   */
  private keys(): ReadonlySet<string> {
    const now = Date.now();
    if (this.cache && (this.watcher || now - this.checked < RECHECK_MS)) return this.cache.keys;
    this.checked = now;
    const stamp = this.stamp();
    if (this.cache?.stamp === stamp) return this.cache.keys;
    const keys = new Set<string>();
    if (stamp && stamp !== "unreadable") {
      let names: string[] = [];
      try { names = readdirSync(this.dir); } catch { /* no favorites readable */ }
      for (const name of names) {
        const model = NAME.test(name) ? readFavorite(this.dir, name) : undefined;
        if (model) keys.add(key(model));
      }
    }
    this.cache = { stamp, keys };
    return keys;
  }

  has(model: ModelRef): boolean { return this.keys().has(key(model)); }

  set(model: ModelRef, favorite: boolean): void {
    if (!valid(model)) throw new Error("a favorite needs a provider and a model id");
    const path = join(this.dir, fileName(model));
    this.cache = undefined;
    if (favorite) {
      if (!this.keys().has(key(model)) && this.keys().size >= this.max) {
        throw new Error(`at most ${this.max} models can be favorites`);
      }
      mkdirSync(this.dir, { recursive: true });
      const temporary = join(this.dir, `.${process.pid}.${randomBytes(6).toString("hex")}.tmp`);
      try {
        writeFileSync(temporary, `${JSON.stringify({ provider: model.provider, id: model.id })}\n`, { mode: 0o600 });
        renameSync(temporary, path);
      } catch (error) {
        rmSync(temporary, { force: true });
        throw error;
      }
    } else {
      rmSync(path, { force: true });
    }
    this.cache = undefined;
    this.changed?.();
  }
}
