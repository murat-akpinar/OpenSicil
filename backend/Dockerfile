# rust:slim-bookworm zaten bir C linker tasir (ring/rustls icin yeterli, ADR-069:
# ekstra sistem paketi gerekmez), musl/alpine hedefi secilmedi.
FROM rust:1.98.1-slim-bookworm AS builder
WORKDIR /app

# sqlx::query! derleme aninda veritabani ister; build asamasinda DB yok,
# .sqlx onbellegi commit'lenir (cargo sqlx prepare)
ENV SQLX_OFFLINE=true

COPY Cargo.toml Cargo.lock ./
COPY .sqlx ./.sqlx
COPY migrations ./migrations
COPY src ./src

RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates wget \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 app

COPY --from=builder /app/target/release/backend /usr/local/bin/backend

USER app
WORKDIR /app
EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/backend"]
