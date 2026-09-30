# nginx-unprivileged: resmi nginx imajinin aksine master process'i root ile calismaz,
# container icinde 8080 dinler (.claude/rules/docker.md)
FROM nginxinc/nginx-unprivileged:1.31.6-alpine

COPY nginx.conf /etc/nginx/nginx.conf

EXPOSE 8080 8443
