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
  messageIndexPath,
} from "./helpers.mjs";
import { startRelayAndPage } from "./relay-dev.mjs";

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const baseUrl = getDefaultBaseUrl();
  const namespace = process.env.MESSAGE_E2E_NAMESPACE ?? `anon/e2e/${Date.now()}`;
  const moqtUrl = getDefaultMoqtUrl();

  const { cleanup, ready } = startRelayAndPage(messageIndexPath);

  try {
    await ready;

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
