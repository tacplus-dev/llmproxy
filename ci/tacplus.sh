#!/usr/bin/env bash
# tacplus as CI runs it (.github/workflows): the compiler image for the floor tacplus.yaml declares
# (`tacplus: "MAJOR.MINOR"`), always that line's latest patch, run on this checkout. The same image the looper's
# `tacplus` runs; it needs no credentials.
#
# Usage:  ci/tacplus.sh build
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
floor="$(sed -n -E 's/^tacplus:[[:space:]]*"?([0-9]+\.[0-9]+)"?.*$/\1/p' "$root/tacplus.yaml" | head -1)"
[[ -n "$floor" ]] || { echo "tacplus.sh: tacplus.yaml declares no tacplus: MAJOR.MINOR floor" >&2; exit 1; }

docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp -v "$root:$root" -w "$root" \
  "ghcr.io/tacplus-dev/tacplus/tacplus:$floor" tacplus "$@"
