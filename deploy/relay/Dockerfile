# Self-hosted Promptify relay. Build from the repository root:
#   docker build -f deploy/relay/Dockerfile -t promptify-relay .
# The relay only forwards end-to-end encrypted frames; put it behind a TLS proxy (see Caddyfile).
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p promptify-relay

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 relay
COPY --from=build /src/target/release/promptify-relay /usr/local/bin/promptify-relay
USER relay
ENV PROMPTIFY_RELAY_LISTEN=0.0.0.0:8787
EXPOSE 8787
STOPSIGNAL SIGINT
HEALTHCHECK --interval=30s --timeout=3s CMD ["/bin/bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/8787"]
ENTRYPOINT ["/usr/local/bin/promptify-relay"]
