#!/usr/bin/env node

import { spawn } from "node:child_process";
import {
  ensureLinuxEnvironment,
  getErrorMessage,
  jsDir,
  repoRoot,
  resolveCommandName,
} from "./media-e2e-helpers.mjs";
import { ensureRelayCertificates } from "./ensure-relay-certs.mjs";

async function main() {
  ensureLinuxEnvironment();
  await assertRequiredTools();
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

async function assertRequiredTools() {
  const checks = [
    ["node", ["--version"]],
    ["npm", ["--version"]],
    ["cargo", ["--version"]],
    ["wasm-pack", ["--version"]],
  ];

  for (const [command, args] of checks) {
    await runCommand(resolveCommandName(command), args, {
      cwd: repoRoot,
      quiet: true,
      errorHint: `Install ${command} before running the media E2E setup.`,
    });
  }
}

async function runCommand(command, args, options) {
  const { cwd, quiet = false, errorHint } = options;
  await new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, args, {
      cwd,
      stdio: quiet ? ["ignore", "ignore", "pipe"] : "inherit",
      env: process.env,
    });

    let stderr = "";
    if (quiet && child.stderr) {
      child.stderr.on("data", (chunk) => {
        stderr += chunk.toString();
      });
    }

    child.on("error", (error) => {
      rejectPromise(
        new Error(
          `${command} failed to start: ${getErrorMessage(error)} ${errorHint ?? ""}`.trim(),
        ),
      );
    });
    child.on("exit", (code) => {
      if (code === 0) {
        resolvePromise();
        return;
      }
      rejectPromise(
        new Error(
          `${command} ${args.join(" ")} exited with code ${code}. ${errorHint ?? ""} ${stderr.trim()}`.trim(),
        ),
      );
    });
  });
}

main().catch((error) => {
  console.error(getErrorMessage(error));
  process.exitCode = 1;
});
