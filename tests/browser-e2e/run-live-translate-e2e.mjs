#!/usr/bin/env node

import { runCommand, spawnProcess, waitForOutput } from "./process.mjs";
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
  jsDir,
  waitForHttpOk,
} from "./helpers.mjs";
import { startRelayAndPage } from "./relay-dev.mjs";

const liveTranslateStreamerPath =
  "/moq-wasm/examples/live-translate/streamer/index.html";
// The temporary-apps service signs a temporary app's token with this secret and the VTS
// verifies it with the same one.
const temporaryAppSecret = "development-temporary-app-secret";
// The ports the pages look for these services at on this machine
// (utils/access.ts, examples/live-translate/liveTranslate.ts).
const temporaryAppsPort = 8083;
const liveTranslatePort = 18090;

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const baseUrl = getDefaultBaseUrl();
  const moqtUrl = getDefaultMoqtUrl();

  process.env.VTS_TEMPORARY_APP_SECRET = temporaryAppSecret;
  const { childProcesses, cleanup, ready } = startRelayAndPage(
    liveTranslateStreamerPath,
  );

  try {
    const temporaryApps = spawnProcess("temporary-apps", "cargo", ["run", "-p", "temporary-apps"], {
      cwd: repoRoot,
      env: {
        ...process.env,
        PORT: String(temporaryAppsPort),
        TEMPORARY_APPS_SECRET: temporaryAppSecret,
      },
    });
    childProcesses.push(temporaryApps);
    await ready;

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
      waitForHttpOk(`http://127.0.0.1:${temporaryAppsPort}/healthz`, 600_000),
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
