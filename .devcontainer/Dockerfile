FROM oven/bun:1.4.2 AS bun

FROM mcr.microsoft.com/devcontainers/javascript-node:4.0.3-24-trixie

COPY --from=bun /usr/local/bin/bun /usr/local/bin/bun
RUN ln -s /usr/local/bin/bun /usr/local/bin/bunx
