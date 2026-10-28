# State: llmproxy

Overwritten to stay current (loopers and agents). Last updated: 2026-10-28 by looper (#10).

- **Version / line:** v0.1.1 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.1.1), line 0.1; tacplus floor 2.4
- **Works:** builds with tacplus 2.4 and `cargo build/test --locked` (48 unit, 12 release tests); release workflow publishes `ghcr.io/tacplus-dev/llmproxy:<version>` and moves `:<MAJOR.MINOR>`
- **Released / deployed:** v0.1.0 and v0.1.1 as images on ghcr.io; no deploy recorded here
- **Known gaps:** `cargo audit` reports two vulnerabilities, both via the generated chassis (#8 crypto-bigint RUSTSEC-2026-0387, #9 rsa RUSTSEC-2023-0071), and four unmaintained-crate warnings (PROPOSALS.md); output-schema field (#2) and budget lanes (#1) open as features
- **Blocked on:** none
