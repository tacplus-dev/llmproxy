# llmproxy

The one place a fleet holds a model-vendor identity. Every service that needs a
model calls this one over S2S; no other service ever holds a model credential.
Authentication to the vendor is workload identity federation (jwt-bearer exchange,
`Config.anthropic_federation_rule_id` and friends), never a static key.

A tacplus service: Rust, generated model in `target/generated-sources`.

## Building

The gates ([ci.yml](.github/workflows/ci.yml)): generate with tacplus at the floor `tacplus.yaml` declares
(`tacplus build`, or `ci/tacplus.sh build` where there is no `tacplus` on the path), then
`cargo build --locked` and `cargo test --locked` (with `protoc` on the path: a dependency's build script runs it).

## Releasing

llmproxy is released by version, and fleets pin a release line, `MAJOR.MINOR`, which is always that line's
highest patch ([tacplus#180](https://github.com/tacplus-dev/tacplus/issues/180)). Versions are `0.x` until
SeaBeam's launch (2026-12-01), when `1.0.0` is cut; on `0.x` a minor may break, a patch never does.

A change merged to main is released by the same run that merged it: a fix as the next patch, a feature as the
next minor. To cut one, from the commit to release:

```sh
git tag v0.1.1          # vMAJOR.MINOR.PATCH; a patch to an older line is tagged on release/MAJOR.MINOR
git push origin v0.1.1
```

The tag runs [release.yml](.github/workflows/release.yml): the gates on the tag, then
`ghcr.io/tacplus-dev/llmproxy:<version>` for linux/amd64 and linux/arm64 ([ci/release-image.sh](ci/release-image.sh)),
labelled with the version and commit (`org.opencontainers.image.version`, `.revision`), and `:<MAJOR.MINOR>`
moved to it only when it is the line's highest patch, so the line tag never goes backwards and no other line
moves ([ci/release-tags.sh](ci/release-tags.sh)). The GitHub release's notes are the commits since the previous
release ([ci/release-notes.sh](ci/release-notes.sh)). A pre-release (`v0.2.0-rc.1`) publishes only its own tag.
Running the workflow by hand (`gh workflow run release.yml -f version=v0.1.1`) is a dry run: it builds both
architectures and prints the tags and notes that version would get, and publishes nothing.

The image runs under the [launcher](https://github.com/tacplus-dev/launcher), built from source at the commit
`LAUNCHER_COMMIT` in release.yml names.

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
