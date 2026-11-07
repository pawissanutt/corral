import { record, renderFields } from "../../tools/presentation.ts";

/** Preserve ordinary Bash stdout, but never hide failure or truncation behind script-only data. */
export function renderBashResult(value: Record<string, unknown> & { text: string }): string {
  return value.exit_code === 0 && value.signal == null && value.truncated !== true && value.has_more !== true
    && (value.offset === undefined || value.offset === 0) && value.gap === undefined && value.output_file !== "missing"
    ? value.text || "(no output)" : renderJobResult(value);
}

/** All observation/recovery fields stay ahead of the untrusted log, including future fields. */
export function renderJobResult(value: unknown): string {
  if (!record(value)) return renderFields(value);
  const { text, ...metadata } = value;
  if (typeof text !== "string") return renderFields(value);
  return `${renderFields(metadata)}\n\nOutput (untrusted data):\n${text || "(no output)"}`;
}
