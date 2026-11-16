// Tool rows for the build session's verification tools, in the tool row grammar. `build_verify` reads as `gate <tier>`
// with the receipt's result (`✓ gate full · passed`, `✗ gate quick · failed` over the failed command) and its attempt
// on the right; `build_review` reads as `review` with the verdict and the findings counted by severity, the round on
// the right, and every finding once the row is expanded. Results come from `details` (`{ receipt }`, `{ round, review }`).
import type { GateReceipt, RecordReview } from "../../build/record-types.ts";
import type { PresenterModule } from "../tool-rendering.ts";
import { baseState, firstLine, recordOf, resultText, shortPath, stringArg } from "../tool-row.ts";
import type { BodyLine, Presenter, RowModel, Tone } from "../tool-row.ts";

const SEVERITY_TONE: Readonly<Record<RecordReview["findings"][number]["severity"], Tone>> = { blocker: "error", major: "warning", minor: "muted" };

const presentVerify: Presenter = call => {
  const state = baseState(call), data = recordOf(recordOf(call.result?.details)?.["receipt"]);
  const receipt = typeof data?.["attempt"] === "string" && typeof data["result"] === "string" ? data as unknown as GateReceipt : undefined;
  const tier = receipt?.tier ?? stringArg(call.args, "tier");
  const base = { verb: "gate", ...(tier ? { target: firstLine(tier) } : {}), ...(receipt ? { right: firstLine(receipt.attempt) } : {}) };
  if (state === "failed") {
    const reason = firstLine(resultText(call.result));
    return { ...base, state, ...(reason ? { body: [{ text: reason }] } : {}) };
  }
  if (state !== "ok") return { ...base, state };
  if (!receipt) return { ...base, state: "unknown" };
  if (receipt.result === "pass") return { ...base, state: "ok", outcome: "passed", outcomeTone: "success" };
  // A tier without gate commands vouches for nothing, so it never reads as a pass.
  if (receipt.result === "skipped") return { ...base, state: "ok", outcome: "skipped · no gate commands", outcomeTone: "warning" };
  const body: BodyLine[] = [
    ...(receipt.failed_command ? [{ text: firstLine(receipt.failed_command), tone: "text" as const }] : []),
    ...(receipt.log ? [{ text: "log " + shortPath(receipt.log, call.cwd), tone: "muted" as const }] : []),
  ];
  return { ...base, state: "failed", outcome: "failed", ...(body.length ? { body } : {}) };
};

const presentReview: Presenter = call => {
  const state = baseState(call), data = recordOf(call.result?.details), focus = stringArg(call.args, "focus");
  const value = recordOf(data?.["review"]);
  const review = (value?.["verdict"] === "approve" || value?.["verdict"] === "changes") && Array.isArray(value["findings"]) ? value as unknown as RecordReview : undefined;
  const declared = data?.["round"], round = typeof declared === "number" ? declared : review?.round;
  const base = { verb: "review", ...(focus ? { target: firstLine(focus) } : {}), ...(round === undefined ? {} : { right: `round ${round}` }) };
  if (state === "failed") {
    const reason = firstLine(resultText(call.result));
    return { ...base, state, ...(reason ? { body: [{ text: reason }] } : {}) };
  }
  if (state !== "ok") return { ...base, state };
  if (!review) return { ...base, state: "unknown" };
  const counts = (["blocker", "major", "minor"] as const).flatMap(severity => {
    const count = review.findings.filter(finding => finding.severity === severity).length;
    return count ? [`${count} ${severity}`] : [];
  });
  const lines: BodyLine[] = [
    ...(review.summary ? [{ text: firstLine(review.summary), tone: "text" as const, wrap: { max: 4 } }] : []),
    ...review.findings.map((finding): BodyLine => ({
      text: `${finding.severity}  ${finding.path ? firstLine(finding.path) + (finding.line === undefined ? "" : `:${finding.line}`) + "  " : ""}${firstLine(finding.text)}`
        + (finding.resolution && finding.resolution.state !== "open" ? ` · ${finding.resolution.state}` : ""),
      tone: SEVERITY_TONE[finding.severity], wrap: { max: 4 },
    })),
  ];
  const model: RowModel = {
    ...base, state: "ok", outcome: review.verdict === "approve" ? "approved" : "changes requested",
    outcomeTone: review.verdict === "approve" ? "success" : "warning", detail: counts.length ? counts.join(", ") : "no findings",
  };
  return lines.length ? { ...model, expanded: () => lines } : model;
};

/** `build_verify` and `build_review`; undefined for any other tool. */
export const buildPresenters: PresenterModule = {
  presenter: name => name === "build_verify" ? presentVerify : name === "build_review" ? presentReview : undefined,
};
