# Workspace image for Chat's Kubernetes sandboxes: Node.js 22, Python 3 with a data
# science stack, C/C++ build tools and git, plus Vite and Next.js starters with
# dependencies installed so a site can start offline.
FROM node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392

RUN apt-get update \
    && apt-get install --yes --no-install-recommends build-essential ca-certificates curl git procps python3 python3-pip python3-venv \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /opt/templates /workspace \
    && chown node:node /opt/templates /workspace \
    && python3 -m venv /opt/python \
    && /opt/python/bin/pip install --no-cache-dir numpy==2.4.6 pandas==3.0.6 matplotlib==3.11.2 \
       scipy==1.17.1 scikit-learn==1.9.1 pytest==9.1.1

ENV NEXT_TELEMETRY_DISABLED=1 npm_config_update_notifier=false npm_config_fund=false npm_config_audit=false \
    PATH=/opt/python/bin:$PATH MPLBACKEND=Agg PYTHONDONTWRITEBYTECODE=1
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
