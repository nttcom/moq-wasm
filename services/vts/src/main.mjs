import { loadApps } from "./apps.mjs";
import { createVtsServer } from "./server.mjs";

const port = Number(process.env.VTS_PORT ?? 8081);
const appsFile = process.env.VTS_APPS_FILE ?? "/etc/vts/apps.json";

const apps = await loadApps(appsFile);
const server = createVtsServer(apps);
server.listen(port, () => {
  console.info(`vts listening on port ${port} (apps: ${apps.size})`);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
