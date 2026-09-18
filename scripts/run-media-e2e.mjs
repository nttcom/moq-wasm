#!/usr/bin/env node

import {
  registerSignalHandlers,
  runCommand,
  spawnProcess,
  spawnViteServer,
  terminateProcess,
  waitForOutput,
} from "./browser-e2e-process.mjs";
import {
  assertE2EPrerequisites,
  ensureLinuxEnvironment,
  getDefaultBaseUrl,
  getDefaultMoqtUrl,
  getDefaultWebPort,
  getErrorMessage,
  jsDir,
  mediaIndexPath,
  repoRoot,
  resolveCommandName,
  waitForHttpOk,
} from "./media-e2e-helpers.mjs";
import { nativeRelayAuthEnv, startVts } from "./vts-dev.mjs";

const childProcesses = [];

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();
  const namespace = process.env.MEDIA_E2E_NAMESPACE ?? `anon/e2e/${Date.now()}`;
  const moqtUrl = getDefaultMoqtUrl();

  const cleanup = async () => {
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

    await runCommand(resolveCommandName("npm"), ["run", "e2e:media"], {
      cwd: jsDir,
      env: {
        ...process.env,
        MEDIA_E2E_BASE_URL: baseUrl,
        MEDIA_E2E_MOQT_URL: moqtUrl,
        MEDIA_E2E_NAMESPACE: namespace,
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
