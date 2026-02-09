# ── Stage 1: Build ────────────────────────────────────────────────
FROM rust:1.85-bookworm AS builder

WORKDIR /app

# Copy manifests first for better Docker layer caching
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs so cargo can fetch + compile dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release && rm -rf src

# Copy the real source code and rebuild
COPY src ./src
RUN touch src/main.rs && cargo build --release

# ── Stage 2: Runtime ─────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/backend ./backend

# Render assigns port 10000 by default
ENV SERVER_HOST=0.0.0.0
ENV SERVER_PORT=10000

EXPOSE 10000

CMD ["./backend"]
