FROM rust:1-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release --locked --bin academic-agent

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell /bin/sh academic

COPY --from=builder /src/target/release/academic-agent /usr/local/bin/academic-agent

RUN mkdir -p /data/inbox /data/archive /data/calendar \
    && chown -R academic:academic /data

USER academic
WORKDIR /data

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/health || exit 1

ENTRYPOINT ["academic-agent"]
CMD ["--config", "/data/academic-agent.toml", "--db", "/data/state.redb", "serve", "--bind", "0.0.0.0:8080", "--with-cron", "--with-watch"]
