#!/usr/bin/env node

import { spawnSync } from "node:child_process";

import {
  ensureLinuxEnvironment,
  getErrorMessage,
  resolveCommandName,
} from "../../scripts/dev-env.mjs";
import { jsDir } from "./helpers.mjs";
import { runCommand } from "./process.mjs";
import { ensureRelayCertificates } from "../../scripts/ensure-relay-certs.mjs";

async function main() {
  ensureLinuxEnvironment();
  assertRequiredTools();
  await ensureRelayCertificates();
  await runCommand(resolveCommandName("npm"), ["ci"], { cwd: jsDir });
  await runCommand(resolveCommandName("npm"), ["run", "wasm"], { cwd: jsDir });
  if (process.env.CI === "true") {
    console.log(
      "Skipping Playwright browser install; CI installs Chromium with Linux dependencies.",
    );
  } else {
    await runCommand(resolveCommandName("npm"), ["run", "e2e:install"], {
      cwd: jsDir,
    });
  }
  console.log("Media E2E setup completed.");
}

function assertRequiredTools() {
  for (const command of ["node", "npm", "cargo", "wasm-pack"]) {
    const { status } = spawnSync(resolveCommandName(command), ["--version"], {
      stdio: "ignore",
    });
    if (status !== 0) {
      throw new Error(
        `${command} --version failed. Install ${command} before running the media E2E setup.`,
      );
    }
  }
}

main().catch((error) => {
  console.error(getErrorMessage(error));
  process.exitCode = 1;
});
