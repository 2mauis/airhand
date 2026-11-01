# syntax=docker/dockerfile:1
# ── 构建 ──────────────────────────────────────────────
FROM rust:1-slim-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY server ./server
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p airhand-server \
 && cp target/release/airhand-server /airhand-server \
 && mkdir -p /data

# ── 运行（distroless，无 shell，非 root）────────────────
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /airhand-server /usr/local/bin/airhand-server
COPY --from=builder --chown=65532:65532 /data /data
ENV AIRHAND_LISTEN=0.0.0.0:4433 \
    AIRHAND_CERT=/data/cert.pem \
    AIRHAND_KEY=/data/key.pem \
    RUST_LOG=info
VOLUME ["/data"]
EXPOSE 4433/udp
USER nonroot
ENTRYPOINT ["/usr/local/bin/airhand-server"]
