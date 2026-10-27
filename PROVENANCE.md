# Provenance

Where llmproxy came from, and the upstream records its code and proto cite. This is the one file that names
the upstream (SeaBeam) and its products: tacplus-dev builds, tests and releases llmproxy with nothing from it,
and nothing outside this file and dated records refers to it (#6, 2026-10-27).

## Upstream design records

Comments in `proto/`, `src/` and `scripts/` cite these records by the short name in the first column. They
live in SeaBeam's private repositories, not here; the decisions they record are restated where they are cited.

| Cited as | Upstream record |
|---|---|
| the design proposal (2026-09-14) | `loopers/scotty/backend/decisions/llm-proxy-design-proposal-2026-09-14.md` |
| the session-auth design (2026-09-14) | `loopers/scotty/backend/decisions/llm-gateway-mcp-session-auth-2026-09-14.md` |
| the slices backlog (2026-09-30) | `loopers/scotty/backend/decisions/backlog-scotty-slices-2026-09-30.md` |
| the harness scenarios (2026-10-02) | `loopers/scotty/backend/decisions/harness-scotty-scenarios-2026-10-02.md` |
| the WIF setup (2026-10-03) | `loopers/scotty/backend/decisions/anthropic-wif-setup-2026-10-03.md` |

The first calling service was SeaBeam's `scotty`; `src/anthropic.rs` was moved from its `src/llm.rs`
(2026-09-15), and `src/main.rs`'s logger comment follows its `src/main.rs`. The hardened base image the
Dockerfile does not use was `registry.gitlab.com/seabeam/schematic/containers/debian/schematic-debian-1-2`;
`proto/dependencies.yaml` was copied from `schematic` `platform/proto/dependencies.yaml` (rust section).
SeaBeam is also the fleet whose launch (2026-12-01) sets when llmproxy cuts `1.0.0` (README, Releasing).

## Provenance and the divergence rule

Copied **wholesale** on 2026-10-23 from `gitlab.com/seabeam/sonar/backend`
(`services/llmproxy`, `proto/services/llmproxy.proto`, and the `proto/shared/crew.proto`
and `proto/shared/pigeon.proto` it imports) at commit `d45d97b`. Jody, 2026-10-23: copy it
wholesale, then *capture any changes in the private version and open issues for the public*.

So, until the private copy consumes this repo:

- The private copy keeps running the fleet. Changes land there first.
- Every change made there is opened as an issue here, so the two cannot silently drift.
- This repo is the home of record in the tacplus registry (service id 75).

## Known divergences from the private copy (2026-10-23)

1. Headers: `Copyright (c) 2026 Jody Scott`, Apache-2.0 (all open-source work is under Jody's
   name). `tacplus.yaml` `pkg` is `com.github.tacplusdev.llmproxy`; the crate is still `llmproxy-model`.
2. Proto sources: `src: proto` only. The service generates alone against its own proto plus the
   sys contract tacplus declares (state host label, `sys/servicehealth.proto`, `sys/minter.proto`).
3. `proto/registry.yaml` is a lock of github.com/tacplus-dev/registry (`global:`), not a
   per-repo allocation.
4. `proto/dependencies.yaml` is the Rust and tools pins copied from the platform proto repo.

## Generates standalone (2026-10-23, evening)

With tacplus branch `registry-and-floor`, `tacplus build` here needs nothing from any other repo.
The notes below record what blocked it earlier in the day and how each was resolved.

## Did not generate standalone earlier that day

`tacplus build` of this repo alone fails on two checks that assume one generation run holds the
whole fleet:

1. `LlmProxyState declares (spec.service_state).label "kernel" but no loaded service serves that
   label` — kernel.proto must be present because kernel serves the state label.
2. With the platform proto added as a source: `(spec.record) declares residency/mutability but no
   (spec.trait) hosts a record query plane` for every platform record — an application proto that
   declares `(spec.trait)` must be present too.

The private copy builds because sonar's `build.sh` merges platform + application + docmanager
protos into one run. Making a service generate against its own proto plus the sys contract, with
the kernel build following dependencies, is the first composition requirement (roadmap item
`tacplus-registry-and-floor`). Until then this repo is the home of record, not the build.

## Open before this repo can be public

- DONE 2026-10-23 (Jody: *"We wont make the change to sonar just yet. just copy it over to its new home
  and make all the changes."*): `crew.proto` and `pigeon.proto` are gone; `BudgetBucket` is declared
  locally with the same values (wire-compatible). Issue #1 still stands for moving the lanes out.
- `auth_services` names Scotty's services. A fleet-neutral allow-list, or configuration, is needed
  before another fleet can use this.
- `GenerateCompletionRequest` has no output-schema field; doppelganger needs schema-constrained
  answers. First feature issue.
