#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

if [[ "$#" -eq 0 ]]; then
  set -- --out "$repo_root/native/by-rs/fixtures/oracle"
elif [[ "$#" -eq 1 && "$1" != --* ]]; then
  set -- --out "$1"
fi

cd "$repo_root"
exec clojure -Sdeps '{:paths ["native/by-rs/oracle/src"]}' \
  -M:dev \
  -m brainyard.native-porting.oracle \
  "$@"
