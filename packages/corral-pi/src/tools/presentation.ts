// Compact text for local tool output.

export function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

/** Keep prose/code literal; nested data retains an unambiguous JSON representation. */
export function renderFields(value: unknown): string {
  if (typeof value === "string") return value;
  if (!record(value)) return JSON.stringify(value) ?? "";
  return Object.entries(value).filter(([, item]) => item !== undefined).map(([key, item]) => {
    const label = /^[a-zA-Z_][a-zA-Z_0-9]*$/.test(key) ? key : JSON.stringify(key);
    return `${label}: ${typeof item === "string" ? item.includes("\n") ? `\n${item}` : item : JSON.stringify(item)}`;
  }).join("\n");
}
