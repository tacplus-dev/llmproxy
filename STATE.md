# State: llmproxy

Overwritten to stay current (loopers and agents). Last updated: 2026-11-06 by looper (#21).

- **Version / line:** v0.1.5 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.1.5), line 0.1, its patches since v0.1.3 cut on release/0.1 (main carries #13, the next minor); tacplus floor 2.6, pinned to 2.6.1 and its build image by digest in tacplus.lock (#21)
- **Works:** builds with tacplus 2.6.1, the release tacplus.lock pins (the generated chassis uses aws-lc-rs, not rsa or the RustCrypto elliptic-curve stack; a newer patch is a commit that moves the lock) and `cargo build/test --locked` (78 unit, 12 release tests); release workflow publishes `ghcr.io/tacplus-dev/llmproxy:<version>` and moves `:<MAJOR.MINOR>`
- **Vendors (#13, on main, unreleased: next minor):** Anthropic, OpenAI (WIF token exchange) and Google Gemini on Vertex AI (GKE Workload Identity); `latest_pattern` routes resolve each vendor's newest model from its listing (cached); every response and `ListModels` carry the resolved `provider_model` and `vendor`. Only Anthropic is live (`LIVE_VENDORS`): OpenAI's and Google's data terms are in the README (#14; OpenAI keeps prompts in its prompt cache up to 24 hours), and both join once their setup exists. xAI waits on keyless auth
- **Released / deployed:** v0.1.0 through v0.1.5 as images on ghcr.io; no deploy recorded here
- **Known gaps:** OpenAI and Google untested against the real APIs (mocks only) and not deployed: the OpenAI identity provider must trust the kernel's JWT issuer, and llmproxy's pod needs a GKE Workload Identity binding (PROPOSALS.md); `cargo audit` reports no vulnerabilities, only four unmaintained-crate warnings (PROPOSALS.md); output-schema field (#2) and budget lanes (#1) open as features
- **Blocked on:** OpenAI and Google going live (#14): Vertex AI on jody1-328023 (durable caching and session resumption off), and an OpenAI identity provider trusting a public kernel issuer (tacplus-dev/fleet-seatrials#4)
