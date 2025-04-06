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

FROM ubuntu:24.04

RUN apt-get update && apt-get install -y ruby-bundler ruby-dev build-essential  \
    bison openssl curl git-core zlib1g zlib1g-dev libssl-dev libyaml-dev  \
    libxml2-dev autoconf libc6-dev ncurses-dev automake libtool postgresql-server-dev-all
WORKDIR /tests
RUN bundle config path ruby
COPY ./tests/ruby/Gemfile .
RUN bundle install

COPY --from=builder /target/release/pg_doorman /usr/bin/pg_doorman
