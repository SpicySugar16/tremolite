# ─── 构建阶段 ─────────────────────────────────────
FROM rust:1.85-slim-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY config.example.toml ./

# 编译（跳过测试和 doc）
RUN cargo build --release 2>&1 && \
    cp target/release/tremolite-cli /build/tremolite && \
    ls -lh /build/tremolite

# ─── 运行阶段 ─────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/tremolite /usr/local/bin/tremolite

WORKDIR /app
VOLUME /app/data /app/logs

EXPOSE 8080

ENV RUST_LOG=info

CMD ["tremolite", "--daemon", "--port", "8080"]
