# syntax=docker/dockerfile:1
FROM --platform=$BUILDPLATFORM rust:1.98.1-bookworm AS builder
ARG TARGETARCH
ARG BUILDARCH
RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) arch=x86_64 ;; arm64) arch=aarch64 ;; *) exit 1 ;; \
    esac; \
    echo "$arch-unknown-linux-gnu" > /rust-target; \
    if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
      apt-get update; \
      apt-get install -y --no-install-recommends "gcc-$arch-linux-gnu" "libc6-dev-$TARGETARCH-cross"; \
      rm -rf /var/lib/apt/lists/*; \
    fi; \
    rustup target add "$(cat /rust-target)"
WORKDIR /src
# rust-toolchain.toml is deliberately not copied: the image already ships the pinned toolchain.
COPY Cargo.toml Cargo.lock build.rs ./
COPY vendor ./vendor
COPY migrations ./migrations
COPY web/dist ./web/dist
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    set -eu; target=$(cat /rust-target); \
    if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
      prefix="${target%-unknown-linux-gnu}-linux-gnu"; \
      export "CARGO_TARGET_$(echo "$target" | tr 'a-z-' 'A-Z_')_LINKER=$prefix-gcc" \
             "CC_$(echo "$target" | tr '-' '_')=$prefix-gcc"; \
    fi; \
    CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --target "$target" --release --locked --bin tgarchive \
    && install -D "target/$target/release/tgarchive" /out/tgarchive \
    && mkdir /out/data

FROM gcr.io/distroless/cc-debian12:nonroot
LABEL org.opencontainers.image.title="tgarchive" \
      org.opencontainers.image.description="Local Telegram message archive: collector, SQLite store, CLI, REST API and web UI" \
      org.opencontainers.image.source="https://github.com/Swind/rust-tgarchive"
COPY --from=builder /out/tgarchive /usr/local/bin/tgarchive
COPY --from=builder --chown=65532:65532 /out/data /data
USER 65532:65532
WORKDIR /data
ENV DATABASE_URL=sqlite:///data/telegram.db \
    TELEGRAM_SESSION_FILE=/data/telegram.session \
    MEDIA_DIR=/data/media \
    SERVER_BIND=0.0.0.0:8080 \
    TGARCHIVE_ALLOW_NON_LOOPBACK=1
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=10s --start-period=15s --retries=3 CMD ["/usr/local/bin/tgarchive", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/tgarchive"]
CMD ["serve"]
