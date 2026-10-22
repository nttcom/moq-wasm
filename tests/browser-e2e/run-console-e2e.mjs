#!/usr/bin/env node

import { copyFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import {
  registerSignalHandlers,
  runCommand,
  spawnProcess,
  terminateProcess,
} from "./process.mjs";
import {
  getErrorMessage,
  repoRoot,
  resolveCommandName,
} from "../../scripts/dev-env.mjs";
import { findFreePort, jsDir, waitForHttpOk } from "./helpers.mjs";
import { startVts, vtsAppsFile, vtsVerifyUrl } from "./vts-dev.mjs";

const consoleWebDir = resolve(repoRoot, "crates", "console", "web");
const childProcesses = [];

async function main() {
  const workDir = mkdtempSync(join(tmpdir(), "console-e2e-"));
  const registryFile = join(workDir, "registry.json");
  copyFileSync(resolve(repoRoot, vtsAppsFile), registryFile);

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
    rmSync(workDir, { recursive: true, force: true });
  };
  registerSignalHandlers(cleanup);

  try {
    const npm = resolveCommandName("npm");
    await runCommand(npm, ["ci"], { cwd: consoleWebDir, env: process.env });
    await runCommand(npm, ["run", "build"], {
      cwd: consoleWebDir,
      env: process.env,
    });

    childProcesses.push(await startVts(registryFile));

    const consolePort = await findFreePort();
    const consoleUrl = `http://127.0.0.1:${consolePort}`;
    childProcesses.push(
      spawnProcess("console", "cargo", ["run", "-p", "console"], {
        cwd: repoRoot,
        env: {
          ...process.env,
          PORT: String(consolePort),
          CONSOLE_DEV_SUB: "e2e-employee",
          CONSOLE_REGISTRY_FILE: registryFile,
          CONSOLE_STATIC_DIR: join(consoleWebDir, "dist"),
        },
      }),
    );
    await waitForHttpOk(`${consoleUrl}/`, 300_000);

    await runCommand(npm, ["run", "e2e:console"], {
      cwd: jsDir,
      env: {
        ...process.env,
        CONSOLE_E2E_BASE_URL: consoleUrl,
        CONSOLE_E2E_VERIFY_URL: vtsVerifyUrl,
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
