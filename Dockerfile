FROM rust:1.87.0-slim-bookworm AS builder

RUN apt-get update && \
    apt-get install -y build-essential pkg-config libssl-dev

COPY . /app
WORKDIR /app
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install  -o Dpkg::Options::=--force-confdef -yq --no-install-recommends postgresql-client openssl \
    # Clean up layer
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/* \
    && truncate -s 0 /var/log/*log
COPY --from=builder /app/target/release/pg_doorman /usr/bin/pg_doorman
COPY --from=builder /app/target/release/patroni_proxy /usr/bin/patroni_proxy
WORKDIR /etc/pg_doorman
ENV RUST_LOG=info
CMD ["pg_doorman"]
STOPSIGNAL SIGINT
