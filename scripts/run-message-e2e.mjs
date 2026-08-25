#!/usr/bin/env node

import {
  registerSignalHandlers,
  runCommand,
  spawnProcess,
  terminateProcess,
  waitForOutput,
} from "./browser-e2e-process.mjs";
import {
  assertPathExists,
  certPath,
  ensureLinuxEnvironment,
  getDefaultBaseUrl,
  getDefaultMoqtUrl,
  getDefaultWebPort,
  getErrorMessage,
  jsDir,
  keyPath,
  messageIndexPath,
  repoRoot,
  resolveCommandName,
  waitForHttpOk,
} from "./media-e2e-helpers.mjs";

const childProcesses = [];

async function main() {
  ensureLinuxEnvironment();
  assertPathExists(
    certPath,
    "TLS certificate",
    "Run node scripts/setup-media-e2e.mjs first.",
  );
  assertPathExists(
    keyPath,
    "TLS private key",
    "Run node scripts/setup-media-e2e.mjs first.",
  );
  assertPathExists(
    `${jsDir}/node_modules`,
    "examples/browser/node_modules",
    "Run node scripts/setup-media-e2e.mjs first.",
  );
  assertPathExists(
    `${jsDir}/pkg/moqt_client_wasm.js`,
    "bindings/wasm build output",
    "Run node scripts/setup-media-e2e.mjs first.",
  );

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();
  const namespace = process.env.MESSAGE_E2E_NAMESPACE ?? `e2e/${Date.now()}`;
  const moqtUrl = getDefaultMoqtUrl();

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
  };

  registerSignalHandlers(cleanup);

  try {
    const server = spawnProcess("server", "cargo", ["run", "-p", "relay"], {
      cwd: repoRoot,
    });
    const vite = spawnProcess(
      "vite",
      resolveCommandName("npm"),
      [
        "exec",
        "vite",
        "--",
        "--host",
        "127.0.0.1",
        "--port",
        String(webPort),
        "--strictPort",
      ],
      { cwd: jsDir },
    );

    childProcesses.push(server, vite);

    await Promise.all([
      waitForOutput(server, /Relay server started/, "relay", 180_000),
      waitForHttpOk(`${baseUrl}${messageIndexPath}`, 120_000),
    ]);

    await runCommand(resolveCommandName("npm"), ["run", "e2e:message"], {
      cwd: jsDir,
      env: {
        ...process.env,
        MEDIA_E2E_BASE_URL: baseUrl,
        MESSAGE_E2E_MOQT_URL: moqtUrl,
        MESSAGE_E2E_NAMESPACE: namespace,
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
