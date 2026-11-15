// The PM's and the assistant's evidence-backed gate diagnosis (recovery authority). The model only
// proposes: Rust records the diagnosis at once where the role's ordinary authority covers it, and
// otherwise stores a bound proposal and answers `proposed`. The owner's decision is a separate
// host-only action sent from tools/define.ts after the pane's confirm dialog; this spec has no
// way to name it, and no retry, approval or authority field.
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";

const Text = Type.String();

const Diagnosis = Type.Object({
  attempt: Type.String({ description: "The failed gate attempt this explains, e.g. the id corral_show reports" }),
  class: Type.Union(["candidate", "red_base", "environment", "flaky", "unknown", "mixed"].map((c) => Type.Literal(c))),
  explanation: Text,
  evidence: Type.Array(Text, { description: "Paths of artifacts attached to the failed attempt that support the diagnosis" }),
  baseline: Type.Optional(Type.Object({ task: Text, run: Text, attempt: Text }, { additionalProperties: false })),
}, { additionalProperties: false });

type GateTriageParams = {
  task: string;
  diagnosis: {
    attempt: string;
    class: string;
    explanation: string;
    evidence: string[];
    baseline?: { task: string; run: string; attempt: string };
  };
};

export const gateTriageTool: CorralToolSpec<GateTriageParams> = {
  name: "corral_gate_triage",
  label: "gate triage",
  description: "Record why one task's retained failed gate attempt failed, from evidence you read: the "
    + "attempt, a class (candidate, red_base, environment, flaky, unknown or mixed), an explanation and "
    + "the evidence: paths of artifacts attached to the failed attempt. A red_base class also needs the "
    + "baseline task, run and attempt that shows the base failing. This records attribution only: it never starts a gate, grants a retry, waives a failure or "
    + "accepts the task, and the retained failed receipt stays. Outcome recorded means it is on the task. "
    + "Outcome proposed means the owner has not approved it yet: corral shows the owner the exact report in "
    + "this pane. Never repeat it automatically, and do not claim it was recorded; carry on and let the owner "
    + "decide. Only if the owner explicitly asks to revisit a pending proposal, re-read the task, confirm the "
    + "same report still fits the current state, and send a fresh call. Declined means "
    + "the owner said no. A refusal is final for that call: re-read the task before another attempt.",
  op: "gate_triage",
  parameters: Type.Object({
    task: Type.String({ description: "One task ID in an approved project" }),
    diagnosis: Diagnosis,
  }, { additionalProperties: false }),
  payload({ task, diagnosis }) {
    const { attempt, explanation, evidence, baseline } = diagnosis;
    return {
      action: "propose",
      task,
      diagnosis: {
        attempt, class: diagnosis.class, explanation, evidence,
        ...(baseline ? { baseline: { task: baseline.task, run: baseline.run, attempt: baseline.attempt } } : {}),
      },
    };
  },
};
