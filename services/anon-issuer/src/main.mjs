import { configFromEnv } from "./config.mjs";
import { createAnonIssuerServer } from "./server.mjs";

const config = configFromEnv(process.env);
const server = createAnonIssuerServer(config);
server.listen(config.port, () => {
  console.info(
    `anon-issuer listening on port ${config.port} (appId: ${config.appId}, ttl: ${config.ttlSeconds}s)`,
  );
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
