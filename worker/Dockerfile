# rust:slim-bookworm zaten bir C linker tasir (ring/rustls icin yeterli, ADR-069:
# ekstra sistem paketi gerekmez), musl/alpine hedefi secilmedi.
FROM rust:1.98.1-slim-bookworm AS builder
WORKDIR /app

# sqlx::query! derleme aninda veritabani ister; build asamasinda DB yok,
# .sqlx onbellegi commit'lenir (cargo sqlx prepare)
ENV SQLX_OFFLINE=true

COPY Cargo.toml Cargo.lock ./
COPY .sqlx ./.sqlx
COPY src ./src

RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10002 app

COPY --from=builder /app/target/release/worker /usr/local/bin/worker

USER app
WORKDIR /app
# worker port acmaz (N-08); saglik "worker-health" alt komutuyla /tmp'deki
# nabiz dosyasindan okunur (ADR-061)
ENTRYPOINT ["/usr/local/bin/worker"]
