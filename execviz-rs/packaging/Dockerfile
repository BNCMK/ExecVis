# A container that carries the tool and nothing else.
#
# The build stage compiles; the run stage holds a single static-ish binary and a
# data directory. Nothing in the image knows about any particular capture: the
# store is a mount, so the container is disposable and the data is not.
FROM rust:1-slim AS build
WORKDIR /src
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM debian:stable-slim
# openssh-client is not optional: SSH-key sign-in verifies signatures with
# ssh-keygen rather than a hand-rolled verifier (spec 2.8)
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates openssh-client \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/execviz /usr/local/bin/execviz
COPY ui.html /usr/local/share/execviz/ui.html

# The tool runs as a non-root user. It reads programs' recordings, which are
# sensitive enough that the process holding them should not be root.
RUN useradd --system --create-home --home-dir /var/lib/execviz execviz
USER execviz
WORKDIR /var/lib/execviz
VOLUME ["/var/lib/execviz"]
EXPOSE 8900

# Liveness, not data. The previous check ran `execviz check` against the capture
# file, which does not exist until the first span arrives; so a fresh container
# reported unhealthy forever and any orchestrator would restart it in a loop.
# Asking whether the server answers is the question a health check is for, and it
# needs no credential because an unauthenticated caller receives the sign-in page.
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s \
  CMD execviz probe "http://127.0.0.1:8900/" >/dev/null || exit 1

ENTRYPOINT ["execviz"]
CMD ["serve", "/var/lib/execviz/capture.db", "--port", "8900", \
     "--collect", "--ui", "/usr/local/share/execviz/ui.html"]
