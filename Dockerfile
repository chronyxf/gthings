FROM rust:alpine AS builder

RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static ca-certificates

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY tests/ ./tests/

RUN cargo build --release --locked -p gthings

FROM alpine:3.20

RUN apk add --no-cache ca-certificates tini

COPY --from=builder /build/target/release/gthings /usr/local/bin/gthings

# Daemon must bind 0.0.0.0 so it is reachable from outside the container.
ENV GTHINGS_SERVE_BIND=0.0.0.0:9080

EXPOSE 9080

ENTRYPOINT ["tini", "--", "gthings", "serve"]