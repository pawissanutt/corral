#!/usr/bin/env bash
# Disposable, credential-free prerequisite probe. Never sends a model request.
set -euo pipefail
exec </dev/null
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"

# Do not pass provider keys, owner configuration, or herdr overrides to the fixture.
if [[ "${1:-}" != "--isolated" ]]; then
  exec env -i PATH="$PATH" HOME="$HOME" \
    bash "$repo/tools/pi-probe/run.sh" --isolated
fi

source scripts/test-herdr.sh
corral_test_herdr_start
export CORRAL_TEST_HERDR_ROOT
trap 'corral_test_herdr_cleanup; printf "fixture_cleanup=removed\n"' EXIT
export PI_CODING_AGENT_DIR="$HOME/.pi/agent"
export PI_OFFLINE=1 PI_SKIP_VERSION_CHECK=1 PI_TELEMETRY=0
printf 'fixture_server=private-plugin-free\n'
printf 'fixture_root=%s\n' "$CORRAL_TEST_HERDR_ROOT"
printf 'fixture_socket=%s/herdr/sessions/corral-test/herdr.sock\n' "$XDG_CONFIG_HOME"
printf 'node=%s npm=%s\n' "$(node --version)" "$(npm --version)"

# Install only in the temporary fixture, with lifecycle scripts disabled.
# npm verifies package-lock integrity against the separately recorded release pin.
npm install --prefix "$CORRAL_TEST_HERDR_ROOT/pi" \
  --registry=https://registry.npmjs.org --ignore-scripts --no-audit --no-fund \
  --userconfig="$CORRAL_TEST_HERDR_ROOT/user.npmrc" \
  --globalconfig="$CORRAL_TEST_HERDR_ROOT/global.npmrc" \
  --cache="$XDG_CACHE_HOME/npm" \
  --progress=false --update-notifier=false \
  @earendil-works/pi-coding-agent@1.3.0
cd "$CORRAL_TEST_HERDR_ROOT"
node "$repo/tools/pi-probe/preflight.mjs" "$CORRAL_TEST_HERDR_ROOT/pi"
