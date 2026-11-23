import type { ChildSnapshot, FleetSnapshot } from "../build/types.ts";
import { basename, isAbsolute, join, relative } from "node:path";
import { parseChildId } from "./remote/input.ts";
import { confinedRegularFile } from "./remote/fleet.ts";
import { readTranscriptHeader } from "./transcript-preview.ts";

export interface TranscriptRecording { file: string; session: string }
export interface RecordingReference { file: string; session?: string }

export function resolveChildRecording(root: string, id: string, reference: RecordingReference): TranscriptRecording | undefined {
  try { parseChildId(id); } catch { return undefined; }
  const path = relative(root, reference.file);
  if (!isAbsolute(reference.file) || !/^(?:children|(?:runtime\/)?auxiliary)\/[^/]+\.jsonl$/.test(path)) return undefined;
  if (path.startsWith("children/") && path !== `children/${id}.jsonl`) return undefined;
  const file = confinedRegularFile(root, reference.file);
  if (!file) return undefined;
  const header = readTranscriptHeader(file);
  if (!header || confinedRegularFile(root, reference.file) !== file) return undefined;
  // Build child files are allocated by exact child ID. Auxiliary filenames instead
  // carry the SDK request identity, which need not be the displayed Explore ID.
  const expected = reference.session ?? (path === `children/${id}.jsonl` ? header
    : /^(?:runtime\/)?auxiliary\//.test(path) ? basename(path, ".jsonl") : undefined);
  return expected === header ? { file, session: header } : undefined;
}

export function retainedReference(root: string, path: unknown, session: unknown): RecordingReference | undefined {
  if (typeof path !== "string" || !/^(?:children|(?:runtime\/)?auxiliary)\/[^/]+\.jsonl$/.test(path)
      || (session !== undefined && (typeof session !== "string" || !session))) return undefined;
  return { file: join(root, path), session };
}

/** The journal describes settled rows only; the current roster owns any duplicate ID. */
export function mergeArchivedChildren(fleet: FleetSnapshot | undefined, archived: readonly ChildSnapshot[]): FleetSnapshot | undefined {
  if (!archived.length) return fleet;
  const current = new Set(fleet?.children.map(child => child.id));
  const children = archived.filter(child => !current.has(child.id)).map(child => ({ ...child, archived: true }));
  return { ...(fleet ?? { mail: [], maxParallel: 0 }), children: [...children, ...(fleet?.children ?? [])] };
}
