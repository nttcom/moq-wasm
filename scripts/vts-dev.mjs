// Development VTS backed by services/vts/apps.example.json, for e2e runners
// that start a native relay. Compose-based runners get the same defaults
// from docker-compose.yml.

import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";

import { spawnProcess } from "./browser-e2e-process.mjs";
import {
  repoRoot,
  resolveCommandName,
  waitForHttpOk,
} from "./media-e2e-helpers.mjs";

export const vtsAppsFile = "services/vts/apps.example.json";
export const relayAppId = "11111111-2222-3333-4444-555555555555";
export const experimentAppId = "ac8adbc8-a2ff-4c41-9f5e-fdaed5e1e65e";
const vtsPort = 8081;
const vtsDir = resolve(repoRoot, "services/vts");

function ensureVtsDependencies() {
  if (!existsSync(resolve(vtsDir, "node_modules"))) {
    execFileSync(resolveCommandName("npm"), ["ci"], {
      cwd: vtsDir,
      stdio: "inherit",
    });
  }
}

// Client tokens must stay within the relay's 24h ttl cap; relay tokens are exempt.
export function mintToken(appId, ttl) {
  ensureVtsDependencies();
  return execFileSync(
    process.execPath,
    [
      "services/vts/bin/mint.mjs",
      "--apps",
      vtsAppsFile,
      "--app-id",
      appId,
      "--publish",
      "",
      "--subscribe",
      "",
      "--ttl",
      ttl,
    ],
    { cwd: repoRoot, encoding: "utf8" },
  ).trim();
}

export async function startVts() {
  ensureVtsDependencies();
  const child = spawnProcess(
    "vts",
    process.execPath,
    ["services/vts/src/main.mjs"],
    {
      cwd: repoRoot,
      env: {
        ...process.env,
        VTS_PORT: String(vtsPort),
        VTS_APPS_FILE: vtsAppsFile,
      },
    },
  );
  await waitForHttpOk(`http://127.0.0.1:${vtsPort}/healthz`, 30_000);
  return child;
}

export function nativeRelayAuthEnv() {
  return {
    AUTH_VTS_URL: `http://127.0.0.1:${vtsPort}/verify`,
    AUTH_RELAY_TOKEN: mintToken(relayAppId, "8760h"),
  };
}
