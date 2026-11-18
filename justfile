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
gates: launcher test
