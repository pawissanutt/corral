#!/usr/bin/env bash
# Opt-in compiler cache for Corral builds. The default is exactly Cargo.
set -euo pipefail

case "${CORRAL_BUILD_CACHE:-off}" in
  off) exec cargo "$@" ;;
  mbx) exec python3 "$(dirname "$0")/cargo_cache.py" "$@" ;;
  *) printf 'Unknown CORRAL_BUILD_CACHE: %s\n' "$CORRAL_BUILD_CACHE" >&2; exit 2 ;;
esac
