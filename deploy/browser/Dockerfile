# Chat's browser: one headless Chromium per person, driven by Chat over the DevTools
# protocol. Fonts cover Latin, CJK and emoji so pages and screenshots read right.
# The same base as the workspace image, so nodes share its layers.
FROM node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392

RUN apt-get update \
    && apt-get install --yes --no-install-recommends chromium ca-certificates curl socat tini \
       fonts-liberation fonts-noto-core fonts-noto-cjk fonts-noto-color-emoji \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /profile \
    && chown node:node /profile
COPY --chmod=755 deploy/browser/start.sh /usr/local/bin/start-browser
USER node
ENV HOME=/tmp/home
EXPOSE 9222
ENTRYPOINT ["tini", "--", "/usr/local/bin/start-browser"]
