/** Maximum UTF-8 length of owner input text. */
export const OWNER_INPUT_TEXT_MAX = 16 * 1024;
/** At most `max` UTF-8 bytes of `text`, cut on a character boundary. */
export function utf8Prefix(text: string, max: number): string {
  const bytes = Buffer.from(text, "utf8");
  if (bytes.length <= max) return text;
  let end = max;
  while (end > 0 && ((bytes[end] ?? 0) & 0xc0) === 0x80) end--;
  return bytes.subarray(0, end).toString("utf8");
}
