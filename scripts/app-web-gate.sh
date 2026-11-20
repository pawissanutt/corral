#!/usr/bin/env bash
# Build the same web package as the installer, without installing or starting it.
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
exec flock /tmp/corral-app-build.lock just app-web
