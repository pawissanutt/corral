#!/bin/sh
export CORRAL_ROOT="$(cd "$(dirname "$0")" && pwd)"
unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID
# A nonempty inventory lets sync reach the queued-dispatch failure under test.
printf '%s\n' '{"id":"fake","result":{"workspaces":[],"panes":[{"pane_id":"w0:p1"}]}}'
