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
  datagramIndexPath,
} from "./helpers.mjs";
import { startRelayAndPage } from "./relay-dev.mjs";

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const baseUrl = getDefaultBaseUrl();
  const moqtUrl = getDefaultMoqtUrl();

  const { cleanup, ready } = startRelayAndPage(datagramIndexPath);

  try {
    await ready;

    await runCommand(resolveCommandName("npm"), ["run", "e2e:datagram"], {
      cwd: jsDir,
      env: {
        ...process.env,
        MEDIA_E2E_BASE_URL: baseUrl,
        DATAGRAM_E2E_MOQT_URL: moqtUrl,
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
