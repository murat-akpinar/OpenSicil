# nginx-unprivileged: resmi nginx imajinin aksine master process'i root ile calismaz,
# container icinde 8080 dinler (.claude/rules/docker.md)
FROM nginxinc/nginx-unprivileged:1.31.6-alpine

# Taban imaj etiketi sabit, paketleri guncel (ADR-071: `Status: fixed` bulgu kalmaz);
# backend/worker'daki `apt-get upgrade` ile ayni kural. Imaj unprivileged kullaniciyla
# gelir, yalnizca bu adim icin root'a cikilir.
USER root
RUN apk upgrade --no-cache
USER nginx

COPY nginx.conf /etc/nginx/nginx.conf
COPY proxy.conf /etc/nginx/proxy.conf
# Backend dusukken sunulan hata sayfalari (binary'deki arayuz CSS'i o an yok)
COPY err/ /usr/share/nginx/html/_err/

EXPOSE 8080 8443
