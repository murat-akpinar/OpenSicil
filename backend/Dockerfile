# rust:slim-bookworm zaten bir C linker tasir (ring/rustls icin yeterli, ADR-069:
# ekstra sistem paketi gerekmez), musl/alpine hedefi secilmedi.
FROM rust:1.98.1-slim-bookworm AS builder
# Repo duzeni imajda birebir korunur: crate /app/backend, arayuz /app/frontend
# (ADR-097). Boylece ../frontend/... goreli yollari iki yerde de ayni dosyayi bulur.
WORKDIR /app/backend

# sqlx::query! derleme aninda veritabani ister; build asamasinda DB yok,
# .sqlx onbellegi commit'lenir (cargo sqlx prepare)
ENV SQLX_OFFLINE=true

COPY Cargo.toml Cargo.lock askama.toml ./
COPY .sqlx ./.sqlx
COPY migrations ./migrations
COPY src ./src
# Arayuz kaynaklari ayri bir adlandirilmis baglamdan gelir (compose:
# additional_contexts.frontend). Baglam koku `.` yapilmadigi icin backend'in
# derleme baglami izole kalir (ADR-070/097). Sablonlar askama ile, CSS/JS/font
# include_bytes!, ceviriler include_str! ile derleme aninda binary'ye gomulur;
# runtime imajinda bu dosyalara gerek yok (ADR-088/089).
COPY --from=frontend . /app/frontend

RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends ca-certificates wget \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 app

COPY --from=builder /app/backend/target/release/backend /usr/local/bin/backend

USER app
WORKDIR /app
# migrate alt komutu migrations/*.sql dosyalarını çalışma zamanında okur
# (sqlx::migrate::Migrator), derleme zamanı makrosu kullanılmaz
COPY --chown=app:app migrations ./migrations
EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/backend"]
