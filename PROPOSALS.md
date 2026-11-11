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
- **ROADMAP.md still names tacplus floor 2.4** (2026-10-29, #11; 2026-11-06, #21; 2026-11-07, #22, where Jody asked for "Now" to name the floor in force): the floor moved to 2.5 (#11)
  and then 2.6, pinned in tacplus.lock (#21; tacplus.yaml, STATE.md), and the current release is v0.1.5, but
  ROADMAP.md's opening line says "Current line v0.1.0; tacplus floor 2.4". ROADMAP.md is Jody's and loopers do not
  edit it, so it needs an agent at his request: "tacplus floor 2.6" (and the current line).
- **A floor patch can turn the gates red with no change here** (2026-10-30, #12; settled by #21): ci/tacplus.sh ran the
  floor's latest patch, and tacplus 2.5.3 changed the generated chassis' dependencies (aws-lc-rs for rsa and the
  elliptic-curve stack, tacplus-dev/tacplus#186), so `cargo build --locked` failed on an untouched main until
  Cargo.lock was re-resolved. A scheduled run of ci.yml (e.g. daily) would catch the next one before a release tag
  does; the platform could also say in tacplus' release notes when a patch changes the generated dependency set.
  From 2.6 (#21) the gates run the release tacplus.lock pins, so a new patch can no longer turn them red; it arrives
  as a commit that moves the lock (`tacplus pin`), and that commit re-resolves Cargo.lock if it must. What stays
  open: nothing here moves the lock when a 2.6.x patch ships (a scheduled `tacplus pin` check, or the nightly scan).
  On 2026-11-09 (nightly scan) tacplus.lock still pinned 2.6.1, three patches behind the line's v2.6.4
  (`gh release list --repo tacplus-dev/tacplus`: v2.6.2, v2.6.3 and v2.6.4, all on 2026-11-08). On 2026-11-10 it still
  pinned 2.6.1, now four behind v2.6.5 (2026-11-09); the lock will have to move anyway for tacplus#368 (#34-#55).
  On 2026-11-11 it still pinned 2.6.1, seven patches behind v2.6.8 (2026-11-11); 2.6 is still the newest line.
  ci.yml's header comment still says the gates "generate with tacplus at the floor tacplus.yaml declares"; it is
  the release tacplus.lock pins now (#21), and a looper cannot push a workflow change (no `workflows` permission).

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
- **A security check in the build, and the README's "Security checks" section** (2026-11-05, nightly scan): the
  build pipeline produces no security report to read. ci.yml runs only generate, build and test, and the README has
  no "Security checks" section, so advisories reach this repository only through the nightly scan's own
  `cargo audit`. On 2026-11-05 that scan found no vulnerabilities, only the four unmaintained crates above. A
  `cargo audit` step in ci.yml (allowing those warnings) would catch an advisory on the PR that brings it in.
  A README section would then say what the step checks and where its report is. The README also has no
  "Trace propagation" section, though the `// trace: unrelated:` markers on the vendor calls cite "(README,
  Vendors)" for "vendor APIs carry no trace_id", and the README does not say that. A sentence there would make the
  citation true (#19, `tacplus-dev/roadmap:decisions/trace-propagation-2026-11-03.md`).
  Update 2026-11-10 (nightly scan): the build pipeline now runs looper-scan on this repository (tacplus-dev/looper
  README, Security checks), and its first report is filed as #27-#56. Still open here: the README's own "Security
  checks" pointer and the README sentence behind the trace markers' citation.
- **A `security-exceptions.yaml` for generated test fixtures** (2026-11-10, nightly scan, #27-#33): gitleaks fails
  every build on seven false positives in tacplus-generated code: JWT-shaped fixtures in the chassis logger's
  redaction tests and PEM armour built at runtime in the service manager. The lasting fix is in tacplus' templates.
  Until it ships, Jody could accept them in a root `security-exceptions.yaml` (id, reason, decided_by, expires), so
  that this repository's builds, the land of #25 among them, are not blocked by code it does not own.
- **The operator's securityContext for llmproxy: 65532** (2026-11-09, #25): the release image now runs as
  `USER 65532:65532` (the UID/GID blobstore#18 chose for the fleet), and llmproxy writes nothing to disk, so it needs
  no volume and no `fsGroup`. The pod the operator renders should set `runAsNonRoot: true`, `runAsUser: 65532` and
  `runAsGroup: 65532` (and may set `readOnlyRootFilesystem: true`), so the cluster enforces what the image declares.
  That change belongs in tacplus-dev/operator.

## Promoted / declined
