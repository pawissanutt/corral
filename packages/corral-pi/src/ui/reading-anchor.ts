// Keeps a paused reader on the same transcript passage when Pi re-wraps it at another width.
// Blank rows separate Pi's transcript blocks (messages, paragraphs, tool cards) at every width, so a
// block index plus a relative depth inside that block survives re-wrapping.
import { stripTerminalSequences } from "@earendil-works/pi-tui";

interface Block { readonly start: number; readonly end: number }

function blocks(lines: readonly string[]): Block[] {
  const result: Block[] = [];
  let start = -1;
  lines.forEach((line, row) => {
    const blank = stripTerminalSequences(line).trim() === "";
    if (!blank && start < 0) start = row;
    if (blank && start >= 0) { result.push({ start, end: row }); start = -1; }
  });
  if (start >= 0) result.push({ start, end: lines.length });
  return result;
}

/** The row of `after` that shows what row `top` of `before` showed. */
export function anchoredRow(before: readonly string[], top: number, after: readonly string[]): number {
  const from = blocks(before), to = blocks(after);
  if (from.length !== to.length || !from.length) {
    return before.length ? Math.round(top * after.length / before.length) : top;
  }
  const index = from.findIndex(block => top < block.end);
  const source = from[index], target = to[index];
  if (!source || !target) {
    const last = from.at(-1), next = to.at(-1);
    return last && next ? next.end + (top - last.end) : top;
  }
  if (top < source.start) return Math.max(to[index - 1]?.end ?? 0, target.start - (source.start - top));
  return target.start + Math.floor((top - source.start) * (target.end - target.start) / (source.end - source.start));
}
