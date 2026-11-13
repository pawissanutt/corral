#!/bin/sh
export CORRAL_ROOT="$(cd "$(dirname "$0")" && pwd)"
unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID
if [ "$1" = '--session' ]; then shift 2; fi
if [ "$1 $2" = 'workspace list' ]; then
  # Health sampling and sync may arrive together; only one may open the one-shot FIFO.
  if mkdir "$CORRAL_READY_PIPE.sent" 2>/dev/null; then
    printf x > "$CORRAL_READY_PIPE"
  fi
  printf '%s\n' '{"result":{"workspaces":[]}}'
else
  printf '%s\n' '{"result":{"panes":[]}}'
fi
