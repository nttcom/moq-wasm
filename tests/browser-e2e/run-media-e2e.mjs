#!/usr/bin/env node

import {
  registerSignalHandlers,
  runCommand,
  spawnProcess,
  spawnViteServer,
  terminateProcess,
  waitForOutput,
} from "./process.mjs";
import {
  ensureLinuxEnvironment,
  getErrorMessage,
  repoRoot,
  resolveCommandName,
} from "../../scripts/dev-env.mjs";
import {
  assertE2EPrerequisites,
  getDefaultBaseUrl,
  getDefaultMoqtUrl,
  getDefaultWebPort,
  jsDir,
  mediaIndexPath,
  startTcpProxy,
  waitForHttpOk,
} from "./helpers.mjs";
import { nativeRelayAuthEnv, startVts } from "./vts-dev.mjs";

const childProcesses = [];

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();
  const namespace = process.env.MEDIA_E2E_NAMESPACE ?? `anon/e2e/${Date.now()}`;
  const throughTcpProxy = process.argv.includes("--through-tcp-proxy");
  let moqtUrl = getDefaultMoqtUrl();
  let expectedTransport = "webtransport";
  let proxy = null;

  const cleanup = async () => {
    proxy?.close();
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
  };

  registerSignalHandlers(cleanup);

  try {
    const vts = await startVts();
    childProcesses.push(vts);
    const server = spawnProcess("server", "cargo", ["run", "-p", "relay"], {
      cwd: repoRoot,
      env: { ...process.env, ...nativeRelayAuthEnv() },
    });
    const vite = spawnViteServer(webPort);

    childProcesses.push(server, vite);

    await Promise.all([
      waitForOutput(server, /Relay server started/, "relay", 180_000),
      waitForHttpOk(`${baseUrl}${mediaIndexPath}`, 120_000),
    ]);

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
