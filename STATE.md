# State: llmproxy

Overwritten to stay current (loopers and agents). Last updated: 2026-10-30 by looper (#12).

- **Version / line:** v0.1.3 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.1.3), line 0.1; tacplus floor 2.5
- **Works:** builds with tacplus 2.5 (its current patches: the generated chassis uses aws-lc-rs, not rsa or the RustCrypto elliptic-curve stack) and `cargo build/test --locked` (48 unit, 12 release tests); release workflow publishes `ghcr.io/tacplus-dev/llmproxy:<version>` and moves `:<MAJOR.MINOR>`
- **Released / deployed:** v0.1.0 through v0.1.3 as images on ghcr.io; no deploy recorded here
- **Known gaps:** `cargo audit` reports no vulnerabilities, only four unmaintained-crate warnings (PROPOSALS.md); output-schema field (#2) and budget lanes (#1) open as features
- **Blocked on:** none
