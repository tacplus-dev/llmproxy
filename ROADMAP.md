# Roadmap: llmproxy

Owned by Jody; changed only by an agent at Jody's request. Loopers propose in PROPOSALS.md.

## Now
The fleet's one model-egress service: holds the federated identity to the model vendor; every other service calls it S2S.
Rust. Current line v0.1.0; tacplus floor 2.4.

## Next
- Nothing approved and unblocked; every open approved issue is in Later.

## Later
- rsa 0.9.10 advisory via the generated chassis (#9), blocked on tacplus-dev/tacplus#186
- crypto-bigint 0.5.5 advisory via the generated chassis (#8), blocked on tacplus-dev/tacplus#186
- Output-schema field on GenerateCompletionRequest, needed by doppelganger (#2, not yet approved; tacplus-dev/roadmap:items/doppelganger-fleet-service.md)
- Move budget lanes out of llmproxy into tacplus-generated code in the owning service (#1, not yet approved; tacplus-dev/roadmap:items/tacplus-registry-and-floor.md)

## Not doing
- Budget lanes in llmproxy: they are the caller's policy, not the proxy's (Jody, 2026-10-23; #1)
- Model egress as a platform primitive: it stays a service (tacplus-dev/roadmap:items/tacplus-registry-and-floor.md)
