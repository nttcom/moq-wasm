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
  waitForHttpOk,
} from "./helpers.mjs";
import { nativeRelayEnv, startVts } from "./vts-dev.mjs";

const liveTranslateStreamerPath =
  "/moq-wasm/examples/live-translate/streamer/index.html";
// The token mint signs a temporary app's token with this secret and the VTS
// verifies it with the same one.
const temporaryAppSecret = "development-temporary-app-secret";
// The ports the pages look for these services at on this machine
// (utils/access.ts, examples/live-translate/liveTranslate.ts).
const tokenMintPort = 8083;
const liveTranslatePort = 18090;
const childProcesses = [];

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();
  const moqtUrl = getDefaultMoqtUrl();

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
  };

  registerSignalHandlers(cleanup);

  try {
    process.env.VTS_TEMPORARY_APP_SECRET = temporaryAppSecret;
    const vts = await startVts();
    const tokenMint = spawnProcess("token-mint", "cargo", ["run", "-p", "token-mint"], {
      cwd: repoRoot,
      env: {
        ...process.env,
        PORT: String(tokenMintPort),
        TOKEN_MINT_SECRET: temporaryAppSecret,
      },
    });
    childProcesses.push(vts, tokenMint);
    const server = spawnProcess("server", "cargo", ["run", "-p", "relay"], {
      cwd: repoRoot,
      env: { ...process.env, ...nativeRelayEnv() },
    });
    const vite = spawnViteServer(webPort);
    childProcesses.push(server, vite);
    await Promise.all([
      waitForOutput(server, /Relay server started/, "relay", 180_000),
      waitForHttpOk(`${baseUrl}${liveTranslateStreamerPath}`, 120_000),
    ]);

    // --echo returns the source audio after each language's fixed delay, so
    // the viewer checks need no Gemini. The browser dials the relay over
    // WebTransport (https://); live-translate dials the same port over QUIC.
    const nativeRelayUrl = moqtUrl.replace(/^https:/, "moqt:");
    const liveTranslate = spawnProcess(
      "live-translate",
      "cargo",
      ["run", "-p", "live-translate", "--", "--relay", nativeRelayUrl, "--insecure", "--echo", "--public-root"],
      { cwd: repoRoot, env: { ...process.env, PORT: String(liveTranslatePort) } },
    );
    childProcesses.push(liveTranslate);
    await Promise.all([
      waitForOutput(liveTranslate, /waiting for broadcasts/, "live-translate", 600_000),
      waitForHttpOk(`http://127.0.0.1:${tokenMintPort}/healthz`, 600_000),
    ]);

    await runCommand(resolveCommandName("npm"), ["run", "e2e:live-translate"], {
      cwd: jsDir,
      env: {
        ...process.env,
        MEDIA_E2E_BASE_URL: baseUrl,
        MEDIA_E2E_MOQT_URL: moqtUrl,
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
