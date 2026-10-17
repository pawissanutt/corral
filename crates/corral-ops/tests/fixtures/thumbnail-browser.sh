#!/bin/sh
for arg do
  case "$arg" in
    --screenshot=*) output=${arg#--screenshot=} ;;
  esac
done
printf '\211PNG\r\n\032\nfixture' > "$output"
