# syntax=docker/dockerfile:1

# ---- build ------------------------------------------------------------------
FROM rust:1.100-slim-bookworm AS build
WORKDIR /app

# Cache dependencies separately from the source.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src ./src
RUN touch src/main.rs src/lib.rs \
    && cargo build --release --locked \
    && strip target/release/tavernkeep \
    && mkdir -p /data

# ---- runtime ----------------------------------------------------------------
# Distroless: glibc, CA certificates and a non-root user, nothing else.
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=build /app/target/release/tavernkeep /usr/local/bin/tavernkeep
# Writable by the distroless `nonroot` user for the optional cache snapshot.
COPY --from=build --chown=65532:65532 /data /data
ENV TK_BIND=0.0.0.0:8080
EXPOSE 8080
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/tavernkeep"]
CMD ["serve"]
