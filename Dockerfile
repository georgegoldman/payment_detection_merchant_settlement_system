# 1. Build Stage
FROM rust:slim-bookworm as builder

WORKDIR /app
COPY . .

# Install Protobuf Compiler (needed for tonic)
RUN apt-get update && apt-get install -y protobuf-compiler

# Build release binary
RUN cargo build --release

# 2. Runtime Stage
FROM debian:bookworm-slim

# Install SSL Certs (Critical for connecting to Sui)
RUN apt-get update && apt-get install -y ca-certificates openssl && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/payment_detection_merchant_settlement_system /app/server

EXPOSE 8000

# Run it
CMD ["./server"]
