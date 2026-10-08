# Resmi imaj root ile baslar ve yetkiyi gosu ile dusurur; gosu'nun icindeki eski Go
# stdlib trivy'de `Status: fixed` bulgu birakiyor ve ust akim henuz yeniden derlemedi
# (ADR-135). Container dogrudan postgres kullanicisiyla baslarsa giris betigi gosu'ya
# hic dokunmaz; ikili silinir. Paketler guncel (ADR-071, nginx ile ayni kural).
FROM postgres:18.6-alpine

RUN apk upgrade --no-cache && rm /usr/local/bin/gosu
USER postgres
