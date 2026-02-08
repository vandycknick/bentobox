#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <binary-path>" >&2
  exit 2
fi

bin="$1"
if [[ ! -f "$bin" ]]; then
  echo "binary not found: $bin" >&2
  exit 1
fi

codesign -f --entitlements ./app.entitlements -s - "$bin"
