# Proposals: llmproxy

Written by loopers. Each proposal: what, why, links. Jody promotes accepted ones into ROADMAP.md (via an agent);
the looper then marks it `promoted` or `declined` here with the date.

## Open
- **Unmaintained crates in the generated chassis** (2026-10-28, #10): `cargo audit` (0.22.2) warns that
  `backoff` 0.4.0 (RUSTSEC-2025-0012) and its `instant` 0.1.13 (RUSTSEC-2024-0384) in chassis/httpclient,
  `rustls-pemfile` 2.2.0 (RUSTSEC-2025-0134) in httpclient, servicemanager and the model, and `unsafe-libyaml`
  0.2.11 (RUSTSEC-2024-0449, via the deprecated `serde_yaml` 0.9) in grpcconfig are unmaintained. None is a
  vulnerability, but they get no fixes. They come from tacplus' generated Rust, so the change belongs to the
  generator in tacplus-dev/tacplus (e.g. `backon` or tokio-retry for backoff, `rustls-pki-types`' PEM parsing for
  rustls-pemfile, a maintained YAML crate for serde_yaml), and this repository picks it up by regenerating.
- **ROADMAP.md still names tacplus floor 2.4** (2026-10-29, #11): the floor moved to 2.5 (tacplus.yaml, STATE.md)
  and the current release is v0.1.2, but ROADMAP.md's opening line says "Current line v0.1.0; tacplus floor 2.4".
  #11 asks for the floor there too; ROADMAP.md is Jody's and loopers do not edit it, so it needs an agent at his
  request: "tacplus floor 2.5" (and the current line).
- **A floor patch can turn the gates red with no change here** (2026-10-30, #12): ci/tacplus.sh runs the floor's
  latest patch, and tacplus 2.5.3 changed the generated chassis' dependencies (aws-lc-rs for rsa and the
  elliptic-curve stack, tacplus-dev/tacplus#186), so `cargo build --locked` failed on an untouched main until
  Cargo.lock was re-resolved. A scheduled run of ci.yml (e.g. daily) would catch the next one before a release tag
  does; the platform could also say in tacplus' release notes when a patch changes the generated dependency set.

- **The nightly scan rechecks xAI for keyless auth** (2026-10-31, #13): Jody's decision on #13 keeps xAI (Grok) out
  until it offers workload identity federation or another keyless method, and asks that "the nightly scan rechecks
  xAI's docs". The nightly scan is tacplus-dev/looper's, so the check belongs there: a finding when xAI documents
  federation, so an issue here can add the adapter (README, Vendors).
- **Deploying OpenAI and Google** (2026-10-31, #13): the adapters are built, but going live needs, beside Jody's
  written data terms (README): an OpenAI Workload Identity Provider that trusts the fleet kernel's JWT issuer (OpenAI
  lists Kubernetes, Google Cloud and SPIFFE; a generic OIDC issuer needs OpenAI support), with a mapping to a
  dedicated, spending-capped project service account, and its three ids in `Config.openai_*`; and, for Vertex AI,
  llmproxy's Kubernetes service account bound to a Google service account with `roles/aiplatform.user`, with
  `Config.vertex_project_id`. Both are operator (deployment) work, not this repository's. If the kernel's issuer is
  not accepted, the alternatives are a GKE projected service-account token or, since OpenAI accepts SPIFFE
  JWT-SVIDs, the fleet's own SPIFFE identity.
- **Per-model prices for `latest` routes** (2026-10-31, #13): a `latest` route's price covers whatever it resolves
  to, so when the latest model's price differs, the route is mispriced until its config changes. A price table
  keyed by provider model in Config, with the route's price as the fallback, would meter each resolved model at
  its own price.
- **Retire `llm_credential`** (2026-10-31, #13): Jody restated the rule on #13, "never a static key", yet Anthropic's
  static-key transition path (D22) is still in the code, used when no federation ids are set. Once every fleet
  runs federation, removing it (and the operator's Secret) would make the rule true of the code.
- **Tools and session turns on OpenAI and Google** (2026-10-31, #13): those vendors are text in, text out today
  (the panel needs no more); tool use and `GenerateSessionTurn` run on Anthropic routes only. With the
  output-schema field (#2), each vendor's structured-output mode could carry the schema instead of the prompt.
- **Go live on OpenAI and Google** (2026-10-31, #14): promoted to ROADMAP.md. The data terms are in the README
  (2026-11-01); Jody dropped the "no extended prompt caching" control and accepted OpenAI's 24-hour cache, so the
  adapter sends no caching parameter (a test pins it). Left: `openai` and `google` in `LIVE_VENDORS` and one real
  panel call per vendor, once Vertex AI is on for jody1-328023 and OpenAI's identity provider trusts a public kernel
  issuer (tacplus-dev/fleet-seatrials#4).
- **`looper-trace check` sees no handlers in llmproxy** (2026-11-03, #19): it reports `pass (0 handlers, 0
  findings)` here, so the vendor, listing and credential calls' `// trace: unrelated:` markers (#19) and the
  `tools/call` frames (#18) are checked only by reading the code. The tool is tacplus-dev/looper's: it should read the
  `GenerateCompletionAsService` and `GenerateSessionTurn` handlers in src/service.rs.

## Promoted / declined
