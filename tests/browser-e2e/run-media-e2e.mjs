#!/usr/bin/env node

import { runCommand } from "./process.mjs";
import {
  ensureLinuxEnvironment,
  getErrorMessage,
  resolveCommandName,
} from "../../scripts/dev-env.mjs";
import {
  assertE2EPrerequisites,
  getDefaultBaseUrl,
  getDefaultMoqtUrl,
  jsDir,
  mediaIndexPath,
  startTcpProxy,
} from "./helpers.mjs";
import { startRelayAndPage } from "./relay-dev.mjs";

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const baseUrl = getDefaultBaseUrl();
  const namespace = process.env.MEDIA_E2E_NAMESPACE ?? `anon/e2e/${Date.now()}`;
  const throughTcpProxy = process.argv.includes("--through-tcp-proxy");
  let moqtUrl = getDefaultMoqtUrl();
  let expectedTransport = "webtransport";
  let proxy = null;

  const { cleanup, ready } = startRelayAndPage(mediaIndexPath, {
    onCleanup: () => proxy?.close(),
  });

  try {
    await ready;

    if (throughTcpProxy) {
      proxy = await startTcpProxy(Number(new URL(moqtUrl).port));
      moqtUrl = `https://127.0.0.1:${proxy.port}`;
      expectedTransport = "websocket";
    }

    await runCommand(resolveCommandName("npm"), ["run", "e2e:media"], {
      cwd: jsDir,
      env: {
        ...process.env,
        MEDIA_E2E_BASE_URL: baseUrl,
        MEDIA_E2E_MOQT_URL: moqtUrl,
        MEDIA_E2E_NAMESPACE: namespace,
        MEDIA_E2E_EXPECTED_TRANSPORT: expectedTransport,
      },
    });
  } finally {
    await cleanup();
  }
}

main().catch((error) => {
  console.error(getErrorMessage(error));
  process.exitCode = 1;
});
