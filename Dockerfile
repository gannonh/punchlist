# The self-host image: the web app's build, `punchlist-server` and `pl`.
FROM node:24-trixie-slim AS web
RUN corepack enable
WORKDIR /src
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY web/package.json web/
RUN pnpm install --frozen-lockfile --filter @punchlist/web
COPY web web
RUN pnpm build

FROM rust:1.99.0-slim-trixie AS rust
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY .sqlx .sqlx
COPY crates crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    SQLX_OFFLINE=true cargo build --release --locked -p punchlist-server -p punchlist-cli \
    && mkdir /out && cp target/release/punchlist-server target/release/pl /out/

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system punchlist \
    && mkdir -p /var/lib/punchlist/proof \
    && chown punchlist /var/lib/punchlist/proof
COPY --from=rust /out/ /usr/local/bin/
COPY --from=web /src/web/dist /usr/share/punchlist/web
ENV PUNCHLIST_BIND=0.0.0.0:7878 \
    PUNCHLIST_WEB_DIR=/usr/share/punchlist/web \
    PUNCHLIST_PROOF_DIR=/var/lib/punchlist/proof
USER punchlist
EXPOSE 7878
ENTRYPOINT ["punchlist-server"]
CMD ["serve"]
