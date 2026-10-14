# Multi-stage build (Sprint W9, DW13): build the SPA, then serve it through the
# Caddy reverse-proxy sidecar (DW1). The mqttaudio daemon runs separately; point
# this at it with DAEMON_HOST (+ DAEMON_TOKEN if it requires auth).
#
#   docker build -t mqttaudio-webui webui
#   docker run -p 8088:8088 \
#     -e DAEMON_HOST=host.docker.internal:8080 \
#     -e DAEMON_TOKEN=your-daemon-token \
#     mqttaudio-webui
#
# Then open http://localhost:8088 and connect with the URL left at /api.

FROM node:20-alpine AS build
WORKDIR /app
RUN corepack enable
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY . .
RUN pnpm build

FROM caddy:2-alpine
COPY --from=build /app/dist /srv/webui
COPY deploy/Caddyfile /etc/caddy/Caddyfile
ENV WEBUI_DIST=/srv/webui \
    SITE_ADDRESS=:8088
EXPOSE 8088
# The base image's default entrypoint runs `caddy run --config /etc/caddy/Caddyfile`.
