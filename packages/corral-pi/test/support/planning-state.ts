// Transport fixture only. Canonical CAS, generation/epoch checks and drain refusals are
// exercised against Rust's real disposable store; host fixtures need current readback fields.
export const PLANNING_FIXTURE = `
const planningFile = join(dir, "planning-boundary.json");
function planningFixtureRead() {
  const path = join(dir, "snapshot.json");
  const state = existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : {};
  state.workflow = existsSync(planningFile) ? JSON.parse(readFileSync(planningFile, "utf8"))
    : state.workflow ?? { workflow: null, phase: "ordinary" };
  state.container = { ...state.container, version: state.workflow.workflow ? 2 : 1 };
  return state;
}
function planningFixtureEvent(envelope) {
  const w = planningFixtureRead().workflow;
  w.boundary ??= { activities: [] };
  const t = envelope.payload.transition;
  if (t.action === "start") w.boundary.activities.push({ id: t.id, tool: t.tool, uncertain: false });
  if (t.action === "settle") w.boundary.activities = w.boundary.activities.filter(a => a.id !== t.id);
  if (t.action === "enter") Object.assign(w, { version: 1, workflow: "W1", phase: "entering",
    owner: { kind: "local" }, state_revision: (w.state_revision ?? 0) + 1, authority_epoch: (w.authority_epoch ?? 0) + 1 });
  if (t.action === "drained") { w.phase = "planning"; w.state_revision++; w.authority_epoch++; }
  writeFileSync(planningFile, JSON.stringify(w));
  return w;
}
`;
