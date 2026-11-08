#!/usr/bin/env bash
# tacplus as CI runs it (.github/workflows): the exact tacplus release and build image that tacplus.lock pins
# (`tacplus pin` writes it), the image by digest, run on this checkout. A newer patch arrives only as a commit that
# moves the lock (tacplus-dev/roadmap decisions/deterministic-builds-2026-10-27.md). It needs no credentials.
#
# Usage:  ci/tacplus.sh build
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
lock="$root/tacplus.lock"
[[ -f "$lock" ]] || { echo "tacplus.sh: no tacplus.lock: run \`tacplus pin\` in the build image of the floor tacplus.yaml declares and commit it" >&2; exit 1; }
image="$(sed -n -E 's/^build_image:[[:space:]]*"?([^"[:space:]]+)"?.*$/\1/p' "$lock" | head -1)"
[[ "$image" == *@sha256:* ]] || { echo "tacplus.sh: tacplus.lock names no build_image by digest" >&2; exit 1; }

docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp -v "$root:$root" -w "$root" \
  "$image" tacplus "$@"
