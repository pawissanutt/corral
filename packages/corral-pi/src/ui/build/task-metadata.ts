import { visibleWidth } from "@earendil-works/pi-tui";
import type { BoardTaskMetadata } from "../../build/types.ts";

/** JSON keeps array boundaries, case, repeated spaces, quotes and backslashes. Escape invisible non-space characters too. */
function tagsLabel(tags: readonly string[]): string {
  return JSON.stringify(tags).replace(/[\p{White_Space}\p{Cc}\p{Cf}]/gu, char => char === " " ? char :
    char.split("").map(unit => "\\u" + unit.charCodeAt(0).toString(16).padStart(4, "0")).join(""));
}

export function taskMetadataLabel({ size, difficulty, tags }: BoardTaskMetadata): string {
  return [size && `size: ${size}`, difficulty && `difficulty: ${difficulty}`,
    tags?.length && `tags: ${tagsLabel(tags)}`].filter(Boolean).join(" · ");
}

const GRAPHEMES = new Intl.Segmenter(undefined, { granularity: "grapheme" });
/** Unlike prose wrapping, cutting at the frame edge does not trim spaces inside quoted labels. */
export function wrapMetadata(text: string, width: number): string[] {
  if (!text || width < 1) return [];
  if (visibleWidth(text) <= width) return [text];
  const lines: string[] = [];
  let line = "", used = 0;
  for (const { segment } of GRAPHEMES.segment(text)) {
    const cells = visibleWidth(segment);
    if (line && used + cells > width) { lines.push(line); line = ""; used = 0; }
    line += segment; used += cells;
  }
  return [...lines, line];
}
