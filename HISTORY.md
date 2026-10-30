# History: llmproxy

Append-only: add entries at the end, never edit or remove one (CI enforces it). One line each:
`- <YYYY-MM-DD> <who: looper|agent|jody> <kind: issue|decision|release|deploy> <one line> (<links>)`

- 2026-10-28 looper issue tokio 1.53.1 (RUSTSEC-2026-0386, AsyncFd I/O-safety unsoundness): bump to 1.54.1 (#10, e6555fc)
- 2026-10-28 looper release v0.1.1 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.1.1)
- 2026-10-29 looper issue tacplus floor 2.4 is behind the latest line 2.5: bump it (McpRequest.tenant and Authorizer signatures change) (#11, 59f153b)
- 2026-10-29 looper release v0.1.2 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.1.2)
- 2026-10-30 looper issue Build gate fails on main: Cargo.lock is stale against the tacplus 2.5.3 chassis (cargo build --locked) (#12, e06e417)
- 2026-10-30 looper release v0.1.3 (https://github.com/tacplus-dev/llmproxy/releases/tag/v0.1.3)
