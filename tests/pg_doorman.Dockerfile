FROM rust:1.75.0-slim-bookworm AS builder

RUN apt-get update && \
    apt-get install -y build-essential pkg-config libssl-dev

# cache
COPY Cargo.toml Cargo.lock ./
COPY patches ./patches
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo fetch
RUN cargo build --release
RUN rm -rf src

# build doorman
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install  -o Dpkg::Options::=--force-confdef -yq --no-install-recommends postgresql-client openssl \
    # Clean up layer
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/* \
    && truncate -s 0 /var/log/*log
COPY --from=builder /target/release/pg_doorman /usr/bin/pg_doorman
WORKDIR /etc/pg_doorman
ENV RUST_LOG=info
CMD ["pg_doorman"]
STOPSIGNAL SIGINT