#!/bin/sh
[ "$1" = '--json' ] && shift
dir=${0%/*}
case "$1 $2" in
  "plugin list") cat "$dir/plugins.json" ;;
  "plugin link")
    [ -f "$dir/allow-link" ] && [ -f "$3/herdr-plugin.toml" ] || exit 1
    printf '%s\n' '{"result":{"linked":true}}'
    ;;
  *) exit 1 ;;
esac
