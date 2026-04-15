#!/usr/bin/env node

import { mkdirSync } from "node:fs";
import { spawn } from "node:child_process";
import {
  certPath,
  ensureLinuxEnvironment,
  getErrorMessage,
  jsDir,
  keyPath,
  repoRoot,
  resolveCommandName,
  serverKeysDir,
} from "./media-e2e-helpers.mjs";

async function main() {
  ensureLinuxEnvironment();
  await assertRequiredTools();
  await ensureCertificates();
  await runCommand(resolveCommandName("npm"), ["install"], { cwd: jsDir });
  await runCommand(resolveCommandName("npm"), ["run", "wasm"], { cwd: jsDir });
  await runCommand(resolveCommandName("npm"), ["run", "e2e:install"], {
    cwd: jsDir,
  });
  console.log("Media E2E setup completed.");
}

async function assertRequiredTools() {
  const checks = [
    ["node", ["--version"]],
    ["npm", ["--version"]],
    ["cargo", ["--version"]],
    ["wasm-pack", ["--version"]],
    ["openssl", ["version"]],
  ];

  for (const [command, args] of checks) {
    await runCommand(resolveCommandName(command), args, {
      cwd: repoRoot,
      quiet: true,
      errorHint: `Install ${command} before running the media E2E setup.`,
    });
  }
}

async function ensureCertificates() {
  if (certPath && keyPath) {
    try {
      await runCommand(
        resolveCommandName("openssl"),
        ["x509", "-in", certPath, "-noout"],
        {
          cwd: repoRoot,
          quiet: true,
        },
      );
      await runCommand(
        resolveCommandName("openssl"),
        ["rsa", "-in", keyPath, "-check", "-noout"],
        {
          cwd: repoRoot,
          quiet: true,
        },
      );
      return;
    } catch (_error) {
      // Regenerate broken certificates below.
    }
  }

  mkdirSync(serverKeysDir, { recursive: true });
  await runCommand(
    resolveCommandName("openssl"),
    [
      "req",
      "-newkey",
      "rsa:2048",
      "-nodes",
      "-keyout",
      keyPath,
      "-x509",
      "-out",
      certPath,
      "-days",
      "365",
      "-subj",
      "/CN=Test Certificate",
      "-addext",
      "subjectAltName = DNS:localhost,IP:127.0.0.1",
    ],
    { cwd: repoRoot },
  );
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
