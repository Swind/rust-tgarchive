# syntax=docker/dockerfile:1
FROM rust:1.98.1-bookworm AS builder
WORKDIR /src
# rust-toolchain.toml is deliberately not copied: the image already ships the pinned toolchain.
COPY Cargo.toml Cargo.lock build.rs ./
COPY vendor ./vendor
COPY migrations ./migrations
COPY web/dist ./web/dist
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin tgarchive \
    && install -D target/release/tgarchive /out/tgarchive

FROM debian:bookworm-slim
LABEL org.opencontainers.image.title="tgarchive" \
      org.opencontainers.image.description="Local Telegram message archive: collector, SQLite store, CLI, REST API and web UI" \
      org.opencontainers.image.source="https://github.com/Swind/rust-tgarchive"
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --user-group --no-create-home --shell /usr/sbin/nologin tgarchive \
    && mkdir /data && chown 10001:10001 /data
COPY --from=builder /out/tgarchive /usr/local/bin/tgarchive
USER 10001:10001
WORKDIR /data
VOLUME /data
ENV DATABASE_URL=sqlite:///data/telegram.db \
    TELEGRAM_SESSION_FILE=/data/telegram.session \
    SERVER_BIND=0.0.0.0:8080 \
    TGARCHIVE_ALLOW_NON_LOOPBACK=1
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=10s --start-period=15s --retries=3 CMD ["tgarchive", "healthcheck"]
ENTRYPOINT ["tgarchive"]
CMD ["serve"]
