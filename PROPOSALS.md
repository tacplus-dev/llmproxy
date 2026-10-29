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

## Promoted / declined
