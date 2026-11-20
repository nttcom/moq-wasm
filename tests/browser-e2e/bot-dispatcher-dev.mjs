// The bot service and the temporary-apps service in front of it, run natively by the browser e2e
// runners whose pages start bots (docs/architecture/bot-dispatcher/architecture.md).

import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { runCommand, spawnProcess } from "./process.mjs";
import { repoRoot, resolveCommandName } from "../../scripts/dev-env.mjs";
import { findFreePort, waitForHttpOk } from "./helpers.mjs";
import { vtsAppsFile, vtsVerifyUrl } from "./vts-dev.mjs";

// The temporary-apps service signs the pages' tokens with this secret, the VTS verifies them with
// it, and the bot service signs the bots' tokens with it. Start the VTS with
// `VTS_TEMPORARY_APP_SECRET` set to it.
export const temporaryAppSecret = "development-temporary-app-secret";

/// Starts `bot-dispatcher` with `botTypes` (its `BOT_DISPATCHER_TYPES`) and temporary-apps, which lets the pages of
/// temporary apps start those types. The VTS must be running. Pushes both processes to
/// `childProcesses` and returns the URL the pages reach temporary-apps at.
export async function startBotDispatcher(botTypes, childProcesses) {
  await runCommand(
    resolveCommandName("cargo"),
    ["build", "-p", "bot-dispatcher", "-p", "temporary-apps"],
    { cwd: repoRoot },
  );
  const dispatcherPort = await findFreePort();
  const dispatcherUrl = `http://127.0.0.1:${dispatcherPort}`;
  const dispatcher = spawnProcess("bot-dispatcher", resolve(repoRoot, "target/debug/bot-dispatcher"), [], {
    cwd: repoRoot,
    env: {
      ...process.env,
      PORT: String(dispatcherPort),
      BOT_DISPATCHER_PUBLIC_URL: dispatcherUrl,
      BOT_DISPATCHER_TYPES: JSON.stringify(botTypes),
      BOT_DISPATCHER_STORE_URL: `${pathToFileURL(mkdtempSync(join(tmpdir(), "bot-dispatcher-e2e-"))).href}/`,
      BOT_DISPATCHER_VTS_URL: vtsVerifyUrl,
      BOT_DISPATCHER_REGISTRY_FILE: resolve(repoRoot, vtsAppsFile),
      BOT_DISPATCHER_TEMPORARY_APP_SECRET: temporaryAppSecret,
    },
  });
  const temporaryAppsPort = await findFreePort();
  const temporaryAppsUrl = `http://127.0.0.1:${temporaryAppsPort}`;
  const temporaryApps = spawnProcess(
    "temporary-apps",
    resolve(repoRoot, "target/debug/temporary-apps"),
    [],
    {
      cwd: repoRoot,
      env: {
        ...process.env,
        PORT: String(temporaryAppsPort),
        TEMPORARY_APPS_SECRET: temporaryAppSecret,
        TEMPORARY_APPS_BOT_DISPATCHER_URL: dispatcherUrl,
        TEMPORARY_APPS_BOT_TYPES: Object.keys(botTypes).join(","),
      },
    },
  );
  childProcesses.push(dispatcher, temporaryApps);
  await Promise.all([
    waitForHttpOk(`${dispatcherUrl}/healthz`, 60_000),
    waitForHttpOk(`${temporaryAppsUrl}/healthz`, 60_000),
  ]);
  return temporaryAppsUrl;
}

/// A bot type the bot service starts as `python -m <module> <args>` from the bot's own environment.
export function localPythonBot(botDir, module, args) {
  return {
    launcher: { kind: "local", program: join(botDir, ".venv", "bin", "python") },
    args: ["-m", module, ...args],
    grace_seconds: 30,
    max_seconds: 3600,
    max_running: 4,
  };
}
