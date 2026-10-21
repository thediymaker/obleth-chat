FROM node:22-alpine@sha256:0a7108bf6c7bf5de370ffb1a3ed6be93d405b43ff159f681a8d18c0e2bc2e402

ENV NODE_ENV=production \
    HOST=0.0.0.0 \
    PORT=4173

WORKDIR /app
COPY --chown=node:node server/ ./server/
COPY --chown=node:node web/ ./web/

USER node
EXPOSE 4173
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD node --input-type=module -e "const response = await fetch('http://127.0.0.1:' + process.env.PORT + '/readyz', { signal: AbortSignal.timeout(3000) }); process.exit(response.ok ? 0 : 1)"

CMD ["node", "server/index.mjs"]
