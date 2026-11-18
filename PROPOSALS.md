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
  On 2026-11-18 (nightly scan) the build's report also lists `chrono` 0.4.45 as unmaintained (RUSTSEC-2026-0543,
  cargo-deny and osv-scanner, non-blocking, not new). In Cargo.lock it comes in through the generated chassis'
  `logger`, `service_im` and `token`, and through `prost-wkt`/`prost-wkt-types` and `pbjson-types`. So it is the
  generator's too (e.g. `jiff` or `time`).
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
  On 2026-11-12 it still pinned 2.6.1, nine patches behind v2.6.10 (2026-11-12); 2.6 is still the newest line.
  On 2026-11-13 it still pinned 2.6.1, ten patches behind v2.6.11 (2026-11-13); 2.6 is still the newest line.
  On 2026-11-15 it still pinned 2.6.1, eleven patches behind v2.6.12 (2026-11-14); 2.6 is still the newest line.
  On 2026-11-16 it still pinned 2.6.1, twelve patches behind v2.6.13 (2026-11-15); 2.6 is still the newest line. No
  2.6 patch carries tacplus-dev/tacplus#368 yet (its backport tacplus-dev/tacplus#390 is unmerged).
  On 2026-11-18 it still pinned 2.6.1, twelve patches behind v2.6.13 (2026-11-15); 2.6 is still the newest line, and
  tacplus-dev/tacplus#390 is still open.
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
  Update 2026-11-12 (nightly scan): the report the nightly scan was handed is of b998d87, the head of the
  `looper/llmproxy-25` branch (#25), not of main (24bbb18). That is why it has no semgrep `missing-user-entrypoint`
  finding even though main's Dockerfile still has no `USER`. The scan should read the report of main's latest build,
  or the report should name its branch (tacplus-dev/looper's).
  Update 2026-11-13 (nightly scan): still so. Today's report is again of b998d87, which is not on main (b9374ef),
  so the build's checks have still not reported on main's Dockerfile. Its Cargo.lock is the same as main's, so the
  dependency findings hold for main. The scan's own `just gates` cannot run in the scan's environment: there is no
  Docker daemon for the tacplus build image (`failed to connect to the docker API at unix:///var/run/docker.sock`).
  So "gates on main" rests on ci.yml's last run on main (43195511918, success, 2026-11-12). The scan needs Docker,
  or the build's own gate result for main (tacplus-dev/looper's).
  Update 2026-11-15 (nightly scan): the report is now of acc06ff, the head of `looper/llmproxy-56` (#56), again not
  main (4797d4e). Its Cargo.lock and Dockerfile are main's, so its findings hold for main (the three new quinn
  advisories are #60), but its eight "accepted" findings rest on that branch's security-exceptions.yaml, which main
  does not have: on main the four unmaintained crates (#56) are not excepted. `just gates` again cannot run here (no
  Docker socket); ci.yml on main, 43405010835 (4797d4e, 2026-11-13), passed.
  Update 2026-11-16 (nightly scan): settled for the report. Today's is of c05f86e, on main (main's head 6a36833 changes
  only HISTORY.md), and its exceptions are main's security-exceptions.yaml. Its 41 findings include none that is `new`
  or blocking. The 22 accepted ones are #34-#55, and the other 19 are #56, #58 and #60. Still open: the scan's own
  `just gates`. There is now a `tacplus` on the path, but it runs the build image through Docker and still fails with
  no socket (`failed to connect to the docker API at unix:///var/run/docker.sock`). ci.yml on main, 43770636130
  (6a36833, 2026-11-15), passed. STATE.md's "Known gaps" still says `cargo audit` reports no vulnerabilities, but
  rustix (#58) and quinn-proto/quinn-udp (#60) have advisories with fixes.
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
- **The security floor deadlocks this repository's lands** (2026-11-15, #25): the build's security job fails the
  pipeline on every HIGH finding it does not except, and main carries two sets owned by different issues: the
  Dockerfile's `missing-user-entrypoint` (#25) and the 22 `cargo-deny:unlicensed:<crate>` findings on the crates
  tacplus generates (#34-#55, fixed upstream in tacplus-dev/tacplus#368, its 2.6 backport #390 unmerged). Each
  issue's land is red on the other's findings (the land of #56 on 2026-11-15 failed the same way), so no change can
  land until a human breaks the tie. Jody's decision (2026-11-15, on #25): the 22 cargo-deny ids are accepted in a
  root `security-exceptions.yaml` (owner tacplus, `decided_by: jodydadescott`, expiring 2026-12-31), landed with #25's
  fix, and dropped in the same land that moves tacplus.lock to the 2.6 patch carrying #368 (#34-#55 stay open until
  then); the same decision covers messaging#18 and gateway#36. Still open: the floor could fail a land only on
  findings its diff adds (the ones `new` since main's report), and leave main's standing findings to their own
  issues (tacplus-dev/looper), so that two issues' findings can never deadlock a repository's lands again.
- **The build's security report has no image scan since 83599bd** (2026-11-17, #63): the report of 83599bd lists
  43 findings, from cargo-deny, osv-scanner and semgrep only. It has no trivy image finding and does not say the image
  scan was skipped. On the same day, the report of df73e20 had the 46 HIGH findings of #63 and the 158 of #64. A
  remote scan of the pinned base (`trivy image --image-src remote --scanners vuln --pkg-types os debian:trixie-slim`,
  still `sha256:a72b4433…`) still finds 47 HIGH: the 46 of #63, unfixed, and zlib1g CVE-2026-85091, fixed upstream and
  covered by #62's build-time upgrade. Jody's decision on #63 relies on the nightly turning these findings blocking
  once Debian fixes them, so a silent gap in the image scan would hide that. The pipeline (tacplus-dev/build,
  looper-scan) should either scan the release image on every build or say in the report that it did not.
  On 2026-11-18 (nightly scan) the image scan is back: the report of 82098cb has 210 trivy findings in the base. But it
  reads the `FROM` by name (`image debian:trixie-slim@sha256:a72b4433…`), not the image the Dockerfile builds. So
  #62's build-time `apt-get upgrade`, which installs zlib1g `1+deb13u1`, is invisible to it, and CVE-2026-85091 fails
  the policy on main again (#67). A scan of the built release image (trivy on the image `docker build` produced, as
  looper#147 does for release jars) would report what ships, so a fix in the Dockerfile would count.
- **The build's checks never read the launcher the release image ships** (2026-11-18, nightly scan, #68):
  `release.yml` builds tacplus-dev/launcher from source at `LAUNCHER_COMMIT` and copies it into the image as PID 1.
  The security checks scan this tree's lockfiles and the base by name, so a vulnerable launcher reaches every
  release without a finding. The pinned d130eab7, built with go1.27.0, has 7 reachable stdlib advisories that only a
  hand scan found. `looper-scan image` already runs `govulncheck -mode=binary` on the looper image's Go binaries
  (looper#167). Running it on the release image's `/usr/local/bin` too, or on the launcher binaries before
  `docker build`, would catch the next one. The pipeline is tacplus-dev/looper's. Since #68, release.yml runs
  `govulncheck -mode=binary` on the launcher binaries before `docker build` and fails the release on a finding: that
  catches it at a release, not between releases.

## Promoted / declined
