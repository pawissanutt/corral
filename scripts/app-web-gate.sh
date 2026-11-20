#!/usr/bin/env bash
# Check the loopback server and build the installer's web package without deploying it.
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
python3 -B scripts/test_app_web_serve.py
exec flock /tmp/corral-app-build.lock just app-web
