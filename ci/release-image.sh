#!/usr/bin/env bash
# Assembles the release image (.github/workflows/release.yml) from the Dockerfile, once per architecture, and
# publishes the two as one multi-architecture image.
#
# Usage:  ci/release-image.sh VERSION REVISION [TAG...]
#
# The Dockerfile takes its binaries from beside it, as the fleet's image pipeline puts them there: for each
# architecture this stages a build context with target/<triple>/release/llmproxy as target/release/llmproxy and
# target/launcher/launcher-linux-<arch>, and builds it labelled with VERSION and REVISION. With TAGs, each
# architecture is pushed by digest and $IMAGE:<TAG> (each TAG) is pointed at both. Without, nothing is pushed:
# the dry run. ARCHS (default "amd64 arm64") narrows it for a local build.
set -euo pipefail

version="$1"
revision="$2"
shift 2
image="${IMAGE:-ghcr.io/tacplus-dev/llmproxy}"
root="$(cd "$(dirname "$0")/.." && pwd)"

digests=()
for arch in ${ARCHS:-amd64 arm64}; do
  case "$arch" in
    amd64) triple=x86_64-unknown-linux-gnu ;;
    arm64) triple=aarch64-unknown-linux-gnu ;;
    *) echo "release-image.sh: unsupported architecture $arch" >&2; exit 1 ;;
  esac
  ctx="$root/target/image/$arch"
  rm -rf "$ctx"
  mkdir -p "$ctx/target/release"
  cp "$root/Dockerfile" "$ctx/"
  cp -R "$root/scripts" "$ctx/scripts"
  cp "$root/target/$triple/release/llmproxy" "$ctx/target/release/llmproxy"
  cp "$root/target/launcher/launcher-linux-$arch" "$ctx/"

  output="type=cacheonly"
  (($#)) && output="type=image,name=$image,push-by-digest=true,name-canonical=true,push=true"
  docker buildx build --platform "linux/$arch" \
    --build-arg "VERSION=$version" --build-arg "REVISION=$revision" \
    --output "$output" --metadata-file "$ctx.json" "$ctx"
  if (($#)); then
    digest="$(jq -r '."containerimage.digest"' "$ctx.json")"
    echo "linux/$arch: $image@$digest"
    digests+=("$image@$digest")
  fi
done

(($#)) || exit 0
refs=()
for tag in "$@"; do refs+=(-t "$image:$tag"); done
docker buildx imagetools create "${refs[@]}" "${digests[@]}"
