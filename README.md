# llmproxy

The one place a fleet holds a model-vendor identity. Every service that needs a
model calls this one over S2S; no other service ever holds a model credential.
Authentication to every vendor is workload identity federation, never a static key (Jody, 2026-10-31,
[#13](https://github.com/tacplus-dev/llmproxy/issues/13): no exception, not even for one vendor). The one key the
code still reads, Anthropic's `Config.llm_credential`, is the transition path from before federation (D22), used
only when no federation ids are set.

## Vendors

llmproxy calls each vendor's **own** API, never a reseller or aggregator (such as GitHub Copilot), so a model id
and a vendor's data terms mean what they say.

| Vendor | API | Credential | Live |
|---|---|---|---|
| Anthropic | Messages API | jwt-bearer exchange of a kernel JWT (`Config.anthropic_federation_rule_id` and friends, or the pod's `ANTHROPIC_*` ids) | yes |
| OpenAI | Chat Completions | RFC 8693 token exchange of a kernel JWT at `auth.openai.com` for a short-lived token of a dedicated OpenAI project service account with a spending cap (`Config.openai_identity_provider_id`, `openai_service_account_id`, `openai_subject_audience`) | no: data terms not recorded |
| Google (Gemini) | Vertex AI `generateContent` | GKE Workload Identity: the pod's Google service account token from the metadata server (`Config.vertex_project_id`, `vertex_location`) | no: data terms not recorded |
| xAI (Grok) | none | **waiting on keyless auth**: xAI documents no workload identity federation, and llmproxy holds no static key. If xAI adds federation (or another keyless method), it can be added then | no |

**Data terms before live traffic.** A vendor takes live traffic only once Jody has recorded here, in writing, that its
API does not train on inputs and keeps them for the shortest retention available. Until then the vendor is off in
code, not only in config: `LIVE_VENDORS` in [src/providers.rs](src/providers.rs) lists the vendors whose terms are
recorded, and a route to any other is refused (FAILED_PRECONDITION) before anything, model listings included, is
sent to it. Turning a vendor on is one reviewed change: its terms in this table, and the vendor in `LIVE_VENDORS`.

| Vendor | No training on API inputs | Retention | Recorded |
|---|---|---|---|
| Anthropic | not recorded here | not recorded here | live before this rule (#13) |
| OpenAI | not yet recorded | not yet recorded | |
| Google | not yet recorded | not yet recorded | |

**Routes.** `Config.models` maps each alias callers use to a vendor and a model. A route pins `provider_model`, or
names `latest_pattern`: the alias then resolves at call time to the vendor's newest model in its own model listing
whose whole id matches (newest by the pattern's numeric capture groups; generally available only, unless
`allow_preview`). Listings are cached for `Config.latest_cache_ttl_ms` (5 minutes). For example:

```yaml
models:
  - {alias: openai-latest, vendor: VENDOR_OPENAI, latest_pattern: 'gpt-(\d+)(?:\.(\d+))?'}
  - {alias: gemini-latest, vendor: VENDOR_GOOGLE, latest_pattern: 'gemini-(\d+)(?:\.(\d+))?-pro'}
  - {alias: claude-opus-latest, vendor: VENDOR_ANTHROPIC, latest_pattern: 'claude-opus-(\d+)(?:-(\d))?-\d{8}'}
```

Every completion and session turn returns the `provider_model` and `vendor` that answered, and `ListModels` returns
what each alias resolves to now, so a caller can record the model and tell when "latest" moved. A `latest` route's
price covers whatever it resolves to. OpenAI and Google routes are text in, text out (no tools); `GenerateSessionTurn`
runs on Anthropic routes only.

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
