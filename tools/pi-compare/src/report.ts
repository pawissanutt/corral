import type { Report } from "./compare.ts";

export function markdown(report: Report): string {
  const lines = [
    "# Pi-native worker comparison",
    "",
    `Data: ${report.data_kind}. Outcome: ${report.outcome}. Complete: ${report.complete}.`,
    "The PM was not evaluated. No default is changed.",
    "",
    "| Pair | Arm | Rounds | Rescues | Needs me | Needs me seconds | No report | PM / owner answers | Gates passed / total | Gate seconds | API-equivalent USD | Tokens | Wall seconds |",
    "| --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- | ---: | ---: | ---: | ---: |",
  ];
  for (const pair of report.pairs) {
    for (const [arm, m] of [["OmO", pair.omo], ["Pi", pair.pi_native]] as const) {
      lines.push(`| ${pair.id} | ${arm} | ${m.review_rounds} | ${m.rescues} | ${m.needs_me} | ${m.needs_me_seconds} | ${m.turns_ended_without_report} | ${m.pm_answers} / ${m.owner_answers} | ${m.gates_passed} / ${m.gate_count} | ${m.gate_seconds} | ${m.api_equivalent_usd ?? "UNPRICED"} | ${m.total_tokens} | ${m.wall_seconds} |`);
    }
    if (pair.invalid.length) lines.push("", `${pair.id} INVALID: ${pair.invalid.join("; ")}.`);
    if (pair.budget_stop) lines.push("", `${pair.id}: STOP, tokens exceeded 2x expected pair tokens.`);
  }
  lines.push("", "## Aggregate and owner thresholds", "", "```json",
    JSON.stringify({ aggregates: report.aggregates, thresholds: report.thresholds }, null, 2),
    "```", "", "## Raw input integrity", "");
  for (const pair of report.pairs) {
    for (const raw of pair.raw) lines.push(`- ${pair.id}: \`${raw.path}\` SHA-256 \`${raw.sha256}\``);
  }
  return `${lines.join("\n")}\n`;
}
