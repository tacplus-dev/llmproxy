# llmproxy

The one place a fleet holds a model-vendor identity. Every service that needs a
model calls this one over S2S; no other service ever holds a model credential.
Authentication to the vendor is workload identity federation (jwt-bearer exchange,
`Config.anthropic_federation_rule_id` and friends), never a static key.

A tacplus service: Rust, generated model in `target/generated-sources`.

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
2. Proto sources: `src: proto` plus the **private** schematic platform proto as an interim second
   source, because `LlmProxyState` is served by kernel and generation needs `kernel.proto`.
   Builds only with access to that repo. Retired when kernel is public or composition lands.
3. `proto/registry.yaml` is a lock of github.com/tacplus-dev/registry (`global:`), not a
   per-repo allocation.
4. `proto/dependencies.yaml` is the Rust and tools pins copied from the platform proto repo.

## Does not generate standalone yet (2026-10-23)

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

- `crew.proto` (1553 lines) and `pigeon.proto` are Scotty product design, imported for one enum,
  `crew.BudgetBucket`. Publishing them publishes that design. Proposed: a local `BudgetBucket`
  with the same values and drop the import (a wire-compatible change, four `use` lines in Rust).
- `auth_services` names Scotty's services. A fleet-neutral allow-list, or configuration, is needed
  before another fleet can use this.
- `GenerateCompletionRequest` has no output-schema field; doppelganger needs schema-constrained
  answers. First feature issue.
