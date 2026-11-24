# History: llmproxy

Append-only: add entries at the end, never edit or remove one (CI enforces it). One line each:
`- <YYYY-MM-DD> <who: looper|agent|jody> <kind: issue|decision|release|deploy> <one line> (<links>)`

- 2026-10-28 looper issue tokio 1.53.1 (RUSTSEC-2026-0386, AsyncFd I/O-safety unsoundness): bump to 1.54.1 (#10, e6555fc)
- 2026-10-28 looper release v0.1.1 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.1.1)
- 2026-10-29 looper issue tacplus floor 2.4 is behind the latest line 2.5: bump it (McpRequest.tenant and Authorizer signatures change) (#11, 59f153b)
- 2026-10-29 looper release v0.1.2 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.1.2)
- 2026-10-30 looper issue Build gate fails on main: Cargo.lock is stale against the tacplus 2.5.3 chassis (cargo build --locked) (#12, e06e417)
- 2026-10-30 looper release v0.1.3 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.1.3)
- 2026-10-31 looper issue Route to OpenAI, xAI and Gemini, resolve each vendor's latest model, return the resolved model id (doppelganger panel mode) (#13, 5d545af)
- 2026-11-02 looper issue Build gate fails on main: Cargo.lock is stale against the tacplus 2.5.6 chassis (cargo build --locked) (#16, ca964e5)
- 2026-11-03 looper issue Trace propagation: vendor and credential calls neither carry trace_id nor have their // trace: unrelated marker (#19, 908c28b)
- 2026-11-07 looper issue tacplus floor 2.5 is behind the latest line 2.6: bump it (2.6 builds from a committed tacplus.lock; there is no tacplus:2.6 image) (#21)
- 2026-11-08 looper issue Pin the build: floor 2.6 and a committed tacplus.lock, so the build pipeline and its security checks run on this repository (#22, 2406f18)
- 2026-11-15 looper issue gitleaks CRITICAL: target/generated-sources/model/s2sm/mod.rs:private-key:1659 at target/generated-sources/model/s2sm/mod.rs:1659 (#33)
- 2026-11-15 looper issue Release image runs as root: semgrep missing-user-entrypoint (HIGH) on Dockerfile:25 fails the build's security checks (#25)
- 2026-11-17 looper issue Release image ships zlib1g 1:1.3.1-1+b1 (CVE-2026-85091 HIGH, blocking; CVE-2026-27171): the Dockerfile never applies Debian's fixed 1+deb13u1 (#62)
- 2026-11-17 looper issue Security checks: 158 new non-blocking findings in the release image (Debian 13 base-package MEDIUM/LOW/UNKNOWN CVEs, DS-0026 no HEALTHCHECK) to fix or except (#64)
- 2026-11-18 looper issue Release image's launcher is built from tacplus-dev/launcher d130eab7 with go1.27.0: 7 reachable stdlib advisories (GO-2026-6859 et al.); launcher v0.1.0 (toolchain go1.27.4) is clean (#68)
- 2026-11-22 looper issue Pinned images behind their lines: relock (tacplus.lock: tacplus 2.6.1 → 2.6.13, and 1 more) (#72)
- 2026-11-23 looper issue url 2.5.8 (RUSTSEC-2026-0592, stack overflow in Url::origin on nested blob: URLs): bump to 2.5.9 (#76)
- 2026-11-23 looper issue STATE.md's Blocked on names closed tacplus-dev/fleet-seatrials#4: rewrite it to what holds #14 now (#77)
- 2026-11-23 looper issue Pinned images behind their lines: relock (tacplus.lock: tacplus 2.6.13 → 2.6.14, and 1 more) (#75)
- 2026-11-23 looper release v0.2.0 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.2.0)
- 2026-11-24 looper issue Release v0.2.0 from main: no release has #13's provider_model on GenerateCompletionResponse, which every doppelganger release calls (#80)
- 2026-11-24 looper release v0.2.1 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.2.1)
- 2026-11-24 looper issue Pinned images behind their lines: relock (Dockerfile: debian:trixie-slim → debian:trixie-slim) (#81)
