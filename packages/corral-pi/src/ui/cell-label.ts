/** Historical transcripts use the old name; this is presentation only. */
export function isJavaScriptCellTool(name: string): boolean {
  return name === "eval_js" || name === "codemode";
}

export const JAVASCRIPT_CELL_TITLE = "JavaScript cell";
