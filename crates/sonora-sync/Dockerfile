FROM rust:1.99.1-bookworm AS build
WORKDIR /workspace
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/
COPY src/ src/
RUN cargo build --locked --release -p sonora-sync

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir /data && chown 10001:10001 /data
COPY --from=build /workspace/target/release/sonora-sync /usr/local/bin/sonora-sync
ENV BIND_ADDR=0.0.0.0:8088 DB_PATH=/data/sonora-sync.sqlite
EXPOSE 8088
VOLUME ["/data"]
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/sonora-sync"]
