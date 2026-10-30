#!/bin/sh
# Live Amazon Bedrock smoke check for an owner with Bedrock access (T-504). It is never part of
# corral's gate or CI: one request is billed per token to the AWS account of the named profile.
# Without --yes-metered it prints the endpoint, profile, region, model and token cap it would use
# and exits non-zero without touching the network. Run `bun install --frozen-lockfile` in
# packages/corral-pi first.
# Usage: scripts/bedrock-smoke.sh --aws-profile <name> --region <region> [--model <id>] [--yes-metered]
set -eu

repo=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
exec bun --no-env-file "$repo/packages/corral-pi/src/auth/bedrock-smoke.ts" "$@" </dev/null
