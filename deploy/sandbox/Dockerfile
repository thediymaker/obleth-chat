# Workspace image for Chat's Kubernetes sandboxes: Node.js 22, Python 3 with a data
# science stack, uv, Jupyter and web frameworks, Rust, Go, C/C++ build tools and git,
# plus Vite and Next.js starters with dependencies installed so a site can start offline.
FROM node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392

RUN apt-get update \
    && apt-get install --yes --no-install-recommends build-essential ca-certificates curl git procps python3 python3-pip python3-venv \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /opt/templates /workspace \
    && chown node:node /opt/templates /workspace \
    && python3 -m venv /opt/python \
    && /opt/python/bin/pip install --no-cache-dir numpy==2.4.6 pandas==3.0.6 matplotlib==3.11.2 \
       scipy==1.17.1 scikit-learn==1.9.1 pytest==9.1.1 uv==0.13.5 jupyterlab==4.6.4 \
       flask==3.1.3 fastapi==0.161.0 uvicorn==0.54.0

# Rust and Go, for the image's architecture. The toolchains and Cargo's registry cache
# belong to the workspace user.
ARG TARGETARCH
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo GOTOOLCHAIN=local
RUN case "${TARGETARCH:-amd64}" in \
      amd64) rust=x86_64 rust_sum=5be191e8c70a5007074254cb74d78dbfdd25cd5a61c27de7b8e5410122ec24f4 \
             go_sum=65efbc80fdc1fa2cd2631cad5b481777a68e8725f34e89f50fc6ac714e379b40 ;; \
      arm64) rust=aarch64 rust_sum=a9c168dbea81a84671c6b7a5c1795cb0eb699020110d107ddfca427bd797e697 \
             go_sum=a06e99309dbcbdca1a8ccd2282c3ee146e55930886e26b4f1193269587165f49 ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && curl -fsSLo /tmp/rustup-init "https://static.rust-lang.org/rustup/archive/1.29.2/${rust}-unknown-linux-gnu/rustup-init" \
    && echo "${rust_sum}  /tmp/rustup-init" | sha256sum -c - \
    && chmod +x /tmp/rustup-init \
    && /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain 1.99.1 --component rustfmt,clippy \
    && rm /tmp/rustup-init \
    && curl -fsSLo /tmp/go.tar.gz "https://go.dev/dl/go1.27.3.linux-${TARGETARCH:-amd64}.tar.gz" \
    && echo "${go_sum}  /tmp/go.tar.gz" | sha256sum -c - \
    && tar -C /usr/local -xzf /tmp/go.tar.gz \
    && rm /tmp/go.tar.gz \
    && chown -R node:node /opt/rustup /opt/cargo

ENV NEXT_TELEMETRY_DISABLED=1 npm_config_update_notifier=false npm_config_fund=false npm_config_audit=false \
    PATH=/opt/python/bin:/opt/cargo/bin:/usr/local/go/bin:$PATH MPLBACKEND=Agg PYTHONDONTWRITEBYTECODE=1
USER node
WORKDIR /opt/templates
RUN npx --yes create-next-app@16.4.2 nextjs --yes --ts --tailwind --eslint --app --no-src-dir \
      --import-alias "@/*" --use-npm --disable-git \
    && rm -f nextjs/next.config.* \
    && npm cache clean --force
COPY --chown=node:node deploy/sandbox/next.config.ts /opt/templates/nextjs/next.config.ts
RUN npm create --yes vite@9.2.1 vite -- --template react-ts --no-interactive \
    && cd vite && rm -rf src/App.css src/assets && npm pkg set name=app \
    && npm install && npm install -D tailwindcss@4.3.3 @tailwindcss/vite@4.3.3 \
    && npm cache clean --force
COPY --chown=node:node deploy/sandbox/vite/ /opt/templates/vite/
WORKDIR /workspace
CMD ["sleep", "infinity"]
