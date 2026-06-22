#!/usr/bin/env node

import { chmodSync, mkdirSync } from "node:fs";
import { spawn } from "node:child_process";
import { pathToFileURL } from "node:url";
import {
  certPath,
  ensureLinuxEnvironment,
  getErrorMessage,
  keyPath,
  repoRoot,
  resolveCommandName,
  serverKeysDir,
} from "./media-e2e-helpers.mjs";

export async function ensureRelayCertificates() {
  ensureLinuxEnvironment();
  await assertOpenSsl();

  if (await hasValidCertificates()) {
    ensureCertificatePermissions();
    return;
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
  ensureCertificatePermissions();
}


async function hasValidCertificates() {
  try {
    await runCommand(resolveCommandName("openssl"), ["x509", "-in", certPath, "-noout"], {
      cwd: repoRoot,
      quiet: true,
    });
    await runCommand(resolveCommandName("openssl"), ["rsa", "-in", keyPath, "-check", "-noout"], {
      cwd: repoRoot,
      quiet: true,
    });
    return true;
  } catch (_error) {
    return false;
  }
}

async function assertOpenSsl() {
  await runCommand(resolveCommandName("openssl"), ["version"], {
    cwd: repoRoot,
    quiet: true,
    errorHint: "Install openssl before running E2E setup.",
  });
}

function ensureCertificatePermissions() {
  // The relay container runs as distroless nonroot and bind-mounts these
  // disposable E2E keys read-only, so make them world-readable for tests.
  chmodSync(certPath, 0o644);
  chmodSync(keyPath, 0o644);
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

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  ensureRelayCertificates().catch((error) => {
    console.error(getErrorMessage(error));
    process.exitCode = 1;
  });
}
