import { createApp, readConfig } from './app.mjs';

let config;
try {
  config = readConfig();
} catch (error) {
  console.error(`obleth-chat: ${error.message}`);
  process.exit(1);
}

const server = createApp({ staticDir: config.staticDir });
server.on('error', error => {
  console.error(`obleth-chat: unable to serve preview (${error.code ?? 'server error'}).`);
  process.exitCode = 1;
});
server.listen(config.port, config.host, () => {
  const host = config.host.includes(':') ? `[${config.host}]` : config.host;
  console.log(`obleth-chat preview: http://${host}:${config.port}`);
  console.log('Preview only: OIDC, gateway, RAG, Kubernetes, and Slurm are not connected.');
});

let stopping = false;
function shutdown() {
  if (stopping) return;
  stopping = true;
  server.close(() => { process.exitCode = 0; });
  // Bound shutdown time even when a client holds a connection open.
  const deadline = setTimeout(() => {
    server.closeAllConnections();
  }, 5_000);
  deadline.unref();
}
process.on('SIGTERM', shutdown);
process.on('SIGINT', shutdown);
