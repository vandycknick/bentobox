#!/usr/bin/env bash
set -euo pipefail

bin="$1"
shift

name="$(basename "$bin")"
if [[ "$name" == "bentoctl" ]]; then
  codesign -f --entitlements ./app.entitlements -s - "$bin"
fi

exec "$bin" "$@"
