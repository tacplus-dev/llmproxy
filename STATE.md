# State: llmproxy

Overwritten to stay current (loopers and agents). Last updated: 2026-11-09 by looper (#25).

- **Version / line:** v0.1.5 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.1.5), line 0.1 (this branch, release/0.1); tacplus floor 2.6, pinned to 2.6.1 and its build image by digest in tacplus.lock (#21)
- **Works:** builds with tacplus 2.6.1, the release tacplus.lock pins (the generated chassis uses aws-lc-rs, not rsa or the RustCrypto elliptic-curve stack; a newer patch is a commit that moves the lock) and `cargo build/test --locked` (48 unit, 12 release tests), together `just gates`, which the build pipeline (tacplus-dev/build) runs in the pinned build image (#22); the release image runs as the fixed non-root UID/GID 65532:65532 (semgrep `p/default` clean on the Dockerfile), and `just gates` checks the Dockerfile's last `USER` is 65532:65532 (#25); release workflow publishes `ghcr.io/tacplus-dev/llmproxy:<version>` and moves `:<MAJOR.MINOR>`
- **Released / deployed:** v0.1.0 through v0.1.5 as images on ghcr.io; no deploy recorded here
- **Known gaps:** `cargo audit` reports no vulnerabilities, only four unmaintained-crate warnings (PROPOSALS.md); the operator should render `runAsUser`/`runAsGroup` 65532 for llmproxy's pods (PROPOSALS.md); output-schema field (#2) and budget lanes (#1) open as features
- **Blocked on:** none
