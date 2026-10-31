# State: llmproxy

Overwritten to stay current (loopers and agents). Last updated: 2026-10-31 by looper (#13).

- **Version / line:** v0.1.3 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.1.3), line 0.1; tacplus floor 2.5
- **Works:** builds with tacplus 2.5 (its current patches: the generated chassis uses aws-lc-rs, not rsa or the RustCrypto elliptic-curve stack) and `cargo build/test --locked` (77 unit, 12 release tests); release workflow publishes `ghcr.io/tacplus-dev/llmproxy:<version>` and moves `:<MAJOR.MINOR>`
- **Vendors (#13, on main, unreleased: next minor):** Anthropic, OpenAI (WIF token exchange) and Google Gemini on Vertex AI (GKE Workload Identity); `latest_pattern` routes resolve each vendor's newest model from its listing (cached); every response and `ListModels` carry the resolved `provider_model` and `vendor`. Only Anthropic is live (`LIVE_VENDORS`): OpenAI and Google stay off until their data terms are recorded in the README. xAI waits on keyless auth
- **Released / deployed:** v0.1.0 through v0.1.3 as images on ghcr.io; no deploy recorded here
- **Known gaps:** OpenAI and Google untested against the real APIs (mocks only) and not deployed: the OpenAI identity provider must trust the kernel's JWT issuer, and llmproxy's pod needs a GKE Workload Identity binding (PROPOSALS.md); `cargo audit` reports no vulnerabilities, only four unmaintained-crate warnings (PROPOSALS.md); output-schema field (#2) and budget lanes (#1) open as features
- **Blocked on:** none
