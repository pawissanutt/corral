#!/bin/sh
# Stands in for the installed corral a latched board hands its pass to; records, never syncs.
dir=$(dirname "$0")
printf '%s\n' "$@" "$HERDR_BIN_PATH" "$CORRAL_HERDR_SESSION" > "$dir/installed-argv"
if [ -e "$dir/installed-fails" ]; then
  echo 'warning: printed before the failure' >&2
  cat "$dir/installed-fails" >&2
  exit 1
fi
