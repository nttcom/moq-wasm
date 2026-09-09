import { loadApps } from "./apps.mjs";
import { createVtsServer } from "./server.mjs";
import {
  DEFAULT_CLOCK_LEEWAY_SECONDS,
  DEFAULT_MAX_TOKEN_TTL_SECONDS,
} from "./verify.mjs";

const port = Number(process.env.VTS_PORT ?? 8081);
const appsFile = process.env.VTS_APPS_FILE ?? "/etc/vts/apps.json";
const maxTokenTtlSeconds = Number(
  process.env.VTS_MAX_TOKEN_TTL_SECONDS ?? DEFAULT_MAX_TOKEN_TTL_SECONDS,
);
const clockLeewaySeconds = Number(
  process.env.VTS_CLOCK_LEEWAY_SECONDS ?? DEFAULT_CLOCK_LEEWAY_SECONDS,
);

const apps = await loadApps(appsFile);
const server = createVtsServer(apps, {
  maxTokenTtlSeconds,
  clockLeewaySeconds,
});
server.listen(port, () => {
  console.info(`vts listening on port ${port} (apps: ${apps.size})`);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
