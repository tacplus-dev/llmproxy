# `just gates`: the checks every change to main passes, as ci.yml runs them and as the build pipeline
# (tacplus-dev/build) runs them in the build image tacplus.lock pins.

# tacplus: the one on the path (the build image's, the release tacplus.lock pins), else ci/tacplus.sh, which runs
# that build image by digest; TACPLUS_BIN overrides both.
TACPLUS := env_var_or_default("TACPLUS_BIN", `command -v tacplus >/dev/null && echo tacplus || echo ci/tacplus.sh`)

default: gates

# Generate target/generated-sources (not committed). Generated code is never committed or edited by hand, so
# generation must leave every tracked file as it found it.
generate:
  #!/usr/bin/env bash
  set -euo pipefail
  before="$(git diff HEAD | sha256sum)"
  {{TACPLUS}} build
  if [[ "$(git diff HEAD | sha256sum)" != "$before" ]]; then
    echo "generate: tacplus build changed tracked files:" >&2; git status --short >&2; exit 1
  fi

# protoc on the path: a dependency's build script (prost-wkt-types) runs it.
build: generate
  cargo build --locked

test: build
  cargo test --locked

# The release image runs as the fixed non-root UID/GID the operator's securityContext names (#25).
image-user:
  #!/usr/bin/env bash
  set -euo pipefail
  user="$(grep -E '^USER ' Dockerfile | tail -n1 | awk '{print $2}' || true)"
  test "$user" = "65532:65532" || { echo "Dockerfile: the last USER must be 65532:65532, not '${user:-root}'" >&2; exit 1; }

# The release image takes Debian's pending security updates at every build (#62): the base tag can lag
# trixie-security by weeks (zlib1g 1+b1, CVE-2026-85091), so the image's apt step upgrades before it installs.
image-upgrade:
  #!/usr/bin/env bash
  set -euo pipefail
  grep -A1 -E '^RUN apt-get update' Dockerfile | tail -n1 | grep -q 'apt-get upgrade -y' \
    || { echo "Dockerfile: the apt step must run 'apt-get upgrade' right after 'apt-get update' (#62)" >&2; exit 1; }

# zlib1g is installed from trixie-security by version (#67): the pinned base ships 1+b1 (CVE-2026-85091), and a
# version, unlike the upgrade, builds the same package every time or fails. It moves with the base digest.
image-zlib:
  #!/usr/bin/env bash
  set -euo pipefail
  grep -A3 -E '^RUN apt-get update' Dockerfile | grep -qF 'zlib1g=1:1.3.dfsg+really1.3.1-1+deb13u1' \
    || { echo "Dockerfile: the apt step must install zlib1g=1:1.3.dfsg+really1.3.1-1+deb13u1 by version (#67)" >&2; exit 1; }

# The release image's base is pinned by digest (#63): a move of the tag cannot change the image, and a base fix
# arrives as a commit that moves the digest, which the build's security checks scan. Every FROM is (#64).
image-base:
  #!/usr/bin/env bash
  set -euo pipefail
  unpinned="$(grep -E '^FROM ' Dockerfile | grep -vE '^FROM [^ ]+@sha256:[0-9a-f]{64}( |$)' || true)"
  test -z "$unpinned" || { echo "Dockerfile: every FROM must be pinned by digest (name:tag@sha256:...), not: $unpinned (#63, #64)" >&2; exit 1; }

# The release image's launcher (PID 1) is pinned to a launcher commit by its full hash, and release.yml fails the
# release unless the launcher it builds is built with the Go its go.mod's toolchain line names and govulncheck
# finds no vulnerability in the binary (#68): the build's security checks never read it.
launcher:
  #!/usr/bin/env bash
  set -euo pipefail
  wf=.github/workflows/release.yml
  grep -qE '^  LAUNCHER_COMMIT: [0-9a-f]{40}$' "$wf" \
    || { echo "$wf: LAUNCHER_COMMIT must name a tacplus-dev/launcher commit by its full hash (#68)" >&2; exit 1; }
  grep -qF 'govulncheck@' "$wf" && grep -qF -- '-mode=binary' "$wf" \
    || { echo "$wf: the release must run govulncheck -mode=binary on the launcher it builds (#68)" >&2; exit 1; }
  grep -qF 'go.mod lacks a toolchain line' "$wf" \
    || { echo "$wf: the release must check the launcher's Go against its go.mod's toolchain line (#68)" >&2; exit 1; }

# The gates every change passes before it is pushed.
gates: image-user image-upgrade image-zlib image-base launcher test
