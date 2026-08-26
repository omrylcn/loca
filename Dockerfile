# ---- build ----
FROM rust:1.94-slim-bookworm AS build
WORKDIR /app
# Cache deps: copy manifests first.
COPY Cargo.toml Cargo.lock ./
COPY crates/protocol/Cargo.toml crates/protocol/
COPY crates/server/Cargo.toml crates/server/
COPY crates/admin/Cargo.toml crates/admin/
# Dummy sources so `cargo build` can resolve+compile deps before real code.
RUN mkdir -p crates/protocol/src crates/server/src crates/admin/src \
 && echo "" > crates/protocol/src/lib.rs \
 && echo "fn main(){}" > crates/server/src/main.rs \
 && echo "fn main(){}" > crates/admin/src/main.rs \
 && cargo build --release --locked -p server
# Now the real sources (web/index.html is include_str!'d by the server).
COPY crates crates
COPY web web
COPY docs docs
COPY PRINCIPLES.md PRINCIPLES.en.md ./
RUN touch crates/server/src/main.rs crates/protocol/src/lib.rs \
 && cargo build --release --locked -p server

# ---- runtime ----
FROM debian:bookworm-slim AS runtime
# rusqlite is bundled (static), so no sqlite lib needed; just certs for good measure.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 10001 loca \
 && useradd --system --uid 10001 --gid loca --home-dir /nonexistent \
      --shell /usr/sbin/nologin loca
WORKDIR /app
COPY --from=build /app/target/release/room-server /usr/local/bin/room-server
# Data dir for the SQLite file (mount a volume here in compose).
RUN install -d -o loca -g loca -m 0700 /data
# 0.0.0.0 inside the container so the published port is reachable; compose
# still maps it to 127.0.0.1 on the host.
ENV PORT=8787 BIND_ADDR=0.0.0.0 DB_PATH=/data/agent-room.db
EXPOSE 8787 3004
USER 10001:10001
CMD ["room-server"]
