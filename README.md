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
the first production launch (2026-12-01), when `1.0.0` is cut; on `0.x` a minor may break, a patch never does.

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

## Provenance

llmproxy was copied from a private upstream on 2026-10-23 and is built here on its own: no source, module,
image or registry it uses is fetched from the upstream. Where it came from, the upstream records its comments
cite, and the divergences from the private copy are in [PROVENANCE.md](PROVENANCE.md).
