FROM rust:1.93.1-bookworm AS api-builder

RUN printf '%s\n' \
    '[source.crates-io]' \
    'replace-with = "rsproxy-sparse"' \
    '[source.rsproxy-sparse]' \
    'registry = "sparse+https://rsproxy.cn/index/"' \
    > /usr/local/cargo/config.toml

WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY apps/api ./apps/api
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p zhiyu-api --bin zhiyu-api \
    && cp /src/target/release/zhiyu-api /tmp/zhiyu-api

FROM node:22-bookworm-slim AS web-builder

RUN corepack enable
WORKDIR /src
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/web/package.json ./apps/web/package.json
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm config set registry https://registry.npmmirror.com \
    && pnpm install --frozen-lockfile --filter @zhiyu/web...
COPY apps/web ./apps/web
RUN pnpm --dir apps/web exec tsc -b \
    && pnpm --dir apps/web exec vite build

FROM debian:bookworm-slim AS runtime

RUN sed -i \
      -e 's|http://deb.debian.org/debian|http://mirrors.tencentyun.com/debian|g' \
      -e 's|http://deb.debian.org/debian-security|http://mirrors.tencentyun.com/debian-security|g' \
      /etc/apt/sources.list.d/debian.sources \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=api-builder /tmp/zhiyu-api /usr/local/bin/zhiyu-api
COPY --from=web-builder /src/apps/web/dist ./web

RUN mkdir -p /data/dev-mail \
    && chown -R nobody:nogroup /app /data

USER nobody:nogroup

ENV APP_ENV=development \
    BIND_ADDR=0.0.0.0:8790 \
    PUBLIC_BASE_URL=https://zhiyu.askfish.net \
    DATABASE_URL=file:/data/preview.db \
    DEV_MAIL_DIR=/data/dev-mail \
    WEB_DIST_DIR=/app/web \
    RUST_LOG=zhiyu_api=info,tower_http=info

EXPOSE 8790

HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=5 \
    CMD curl --fail --silent http://127.0.0.1:8790/health/ready >/dev/null || exit 1

ENTRYPOINT ["zhiyu-api"]
