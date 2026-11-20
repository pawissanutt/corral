/** The most a child's brief may hold (`FleetRuntime.start` refuses a longer one). */
export const BRIEF_LIMIT = 32768;

export interface ReviewBriefParts {
  /** What the reviewer always needs, in order before the variable sections. */
  head: string[];
  /** The owner's prompts, verbatim while they fit. */
  prompts: string[];
  /** One entry per slice: its id, title, owned paths and report. */
  slices: string[];
  /** What follows the variable sections: the gate, the focus, how to finish. */
  tail: string[];
  /** Where the full prompts and reports stay, named when any of them was shortened. */
  record: string;
}

const JOIN = "\n\n";

/**
 * The reviewer's brief within `BRIEF_LIMIT`. A long build's slice reports and prompts together
 * outgrew the limit, and the fleet refused the reviewer, so no build of that size could be
 * reviewed. The head and tail stay whole; the prompts and slices share what is left, each
 * shortened only as far as the others leave no room for it (the shortest stay whole), with a
 * note saying how much went and where the whole text is.
 */
export function reviewBrief(parts: ReviewBriefParts): string {
  const fixed = [...parts.head, "Owner prompts (verbatim):", "Slices:", ...parts.tail];
  const items = [...parts.prompts, ...parts.slices];
  const notice = `Some prompts or reports below are shortened; the whole text is in ${parts.record}.`;
  const overhead = fixed.join(JOIN).length + JOIN.length * items.length + JOIN.length + notice.length;
  const room = BRIEF_LIMIT - overhead;
  if (room < 0) throw new Error(`The review brief needs ${overhead - notice.length} characters before any prompt or report; shorten the focus.`);
  const total = items.reduce((sum, item) => sum + item.length, 0);
  const fitted = total <= room ? items : shorten(items, room);
  const prompts = fitted.slice(0, parts.prompts.length);
  const slices = fitted.slice(parts.prompts.length);
  return [...parts.head, ...(total <= room ? [] : [notice]), "Owner prompts (verbatim):", ...prompts, "Slices:", ...slices, ...parts.tail]
    .join(JOIN);
}

/** Each item cut to one common cap, the largest that fits `room` with every cut's note. */
function shorten(items: string[], room: number): string[] {
  const cut = (item: string, cap: number) => {
    if (item.length <= cap) return item;
    const note = `\n[${item.length - cap} characters omitted]`;
    return item.slice(0, Math.max(0, cap - note.length)) + note;
  };
  const size = (cap: number) => items.reduce((sum, item) => sum + cut(item, cap).length, 0);
  let low = 0;
  let high = Math.max(...items.map(item => item.length));
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    if (size(mid) <= room) low = mid; else high = mid - 1;
  }
  return items.map(item => cut(item, low));
}
