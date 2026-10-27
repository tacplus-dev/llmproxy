# Public base, deliberately and visibly: tacplus-dev builds with nothing from a
# private registry (PROVENANCE.md names the hardened base this replaces).
FROM debian:trixie-slim

# The generated ServiceManager talks TLS to kernel, and this service also
# makes an outbound TLS call of its own to api.anthropic.com — the ONE
# outbound credential-bearing call in the whole fleet (see src/anthropic.rs).
# libssl is already present in trixie-slim; ca-certificates is not.
RUN apt-get update -qq \
 && apt-get install -y -qq --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# No grpc_health_probe: the operator renders tcpSocket probes on 8443
# (grpcProbe8443 in the chart), so the binary would be dead weight.
COPY target/release/llmproxy /usr/local/bin/llmproxy
# The harness-scripted stand-in's scripts (src/scripted.rs SCRIPTS_DIR).
COPY scripts /usr/share/llmproxy/scripts

# Every service runs under the launcher (github.com/tacplus-dev/launcher): PID 1
# that forwards signals, propagates the exit code, and kills a deadlocked child
# on gRPC health. The fleet's image pipeline puts the binary beside the Dockerfile.
ARG TARGETARCH
COPY launcher-linux-${TARGETARCH} /usr/local/bin/launcher
ENV LAUNCH_CMD="llmproxy" HEALTH_PORT=8443
ENTRYPOINT ["launcher"]

# Set by the release (ci/release-image.sh): the version without its v, and the commit.
ARG VERSION=dev
ARG REVISION=unknown
LABEL org.opencontainers.image.source="https://github.com/tacplus-dev/llmproxy" \
      org.opencontainers.image.version="$VERSION" \
      org.opencontainers.image.revision="$REVISION"
