// Input records from corral-core/{progress,completion}.rs. Rust remains authoritative
// for identity, CAS revisions, content digests and state-dependent invariants.
import { Type } from "typebox";

const Text = Type.String();
const Strings = Type.Array(Text);
const Revision = Type.Integer({ minimum: 0, maximum: Number.MAX_SAFE_INTEGER });
const ObjectOptions = { additionalProperties: false };
const Identity = { task: Text, run: Text, spec_revision: Text, expected_revision: Revision };
const Step = Type.Object({
  id: Text, label: Text,
  status: Type.Union([
    ...["pending", "in_progress", "done"].map((state) => Type.Object({ state: Type.Literal(state) }, ObjectOptions)),
    Type.Object({ state: Type.Literal("dropped"), reason: Text }, ObjectOptions),
  ]),
  criteria: Type.Optional(Strings),
}, ObjectOptions);

export const ProgressUpdate = Type.Object({
  ...Identity,
  phases: Type.Array(Type.Object({ id: Text, label: Text, steps: Type.Array(Step) }, ObjectOptions)),
}, ObjectOptions);

export const CheckpointUpdate = Type.Object({
  ...Identity, progress_revision: Revision, head: Text, base: Text, tree: Text,
  criterion_ids: Strings, evidence_refs: Strings, next_action: Text, decisions: Strings,
  waits: Type.Array(Type.Object({
    id: Text, owner: Text, handle: Text, condition: Text, resolution: Type.Optional(Text),
  }, ObjectOptions)),
  resources: Type.Array(Type.Object({
    id: Text, owner: Text, description: Text, cleanup: Text, resolution: Type.Optional(Text),
  }, ObjectOptions)),
}, ObjectOptions);

export const EvidenceObservation = Type.Object({
  version: Type.Literal(1), key: Text, criterion: Text, run: Text, attempt: Text,
  status: Type.Union([Type.Literal("pass"), Type.Literal("fail"), Type.Literal("not_run")]),
  action: Text, environment: Text, expected: Text, observed: Text, artifacts: Strings,
  captured: Type.String({ format: "date-time" }), producer: Text, live: Type.Boolean(),
  cleanup: Type.Optional(Text),
}, ObjectOptions);
