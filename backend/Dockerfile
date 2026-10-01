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
# askama sablonlarini derleme aninda Rust koduna gomer (askama_derive); runtime
# imajinda templates/ dizinine gerek yok.
COPY templates ./templates
# Statik varliklar da derleme aninda binary'ye gomulur (ADR-088: include_bytes!);
# app.css kaynaktan uretilip commit'lenir (sh scripts/build-css.sh).
COPY static ./static

RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends ca-certificates wget \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 app

COPY --from=builder /app/target/release/backend /usr/local/bin/backend

USER app
WORKDIR /app
# migrate alt komutu migrations/*.sql dosyalarını çalışma zamanında okur
# (sqlx::migrate::Migrator), derleme zamanı makrosu kullanılmaz
COPY --chown=app:app migrations ./migrations
EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/backend"]
