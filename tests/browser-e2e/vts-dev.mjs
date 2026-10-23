// Development VTS backed by crates/vts/apps.example.json, run natively by
// the e2e runners. Runners that use the compose vts service get the same
// defaults from docker-compose.yml.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { spawnProcess } from "./process.mjs";
import { repoRoot } from "../../scripts/dev-env.mjs";
import { waitForHttpOk } from "./helpers.mjs";

export const vtsAppsFile = "crates/vts/apps.example.json";
export const relayAppId = "11111111-2222-3333-4444-555555555555";
export const experimentAppId = "ac8adbc8-a2ff-4c41-9f5e-fdaed5e1e65e";
export const vtsPort = Number(process.env.VTS_PORT ?? 8081);
export const vtsVerifyUrl = `http://127.0.0.1:${vtsPort}/verify`;
const vtsBinary = resolve(repoRoot, "target/debug/vts");
const authTokenBinary = resolve(repoRoot, "target/debug/auth-token");

let binariesBuilt = false;

function ensureBinariesBuilt() {
  if (!binariesBuilt) {
    execFileSync("cargo", ["build", "-p", "vts"], {
      cwd: repoRoot,
      stdio: "inherit",
    });
    execFileSync("cargo", ["build", "-p", "auth-token", "--features", "cli"], {
      cwd: repoRoot,
      stdio: "inherit",
    });
    binariesBuilt = true;
  }
}

function secretOf(appId) {
  const apps = JSON.parse(readFileSync(resolve(repoRoot, vtsAppsFile), "utf8"));
  return apps.find((app) => app.appId === appId).secret;
}

// Client tokens must stay within the relay's 24h ttl cap; relay tokens are exempt.
export function mintToken(appId, ttl) {
  ensureBinariesBuilt();
  return execFileSync(
    authTokenBinary,
    ["--app-id", appId, "--publish", "", "--subscribe", "", "--ttl", ttl],
    {
      cwd: repoRoot,
      encoding: "utf8",
      env: { ...process.env, AUTH_TOKEN_SECRET: secretOf(appId) },
    },
  ).trim();
}

export async function startVts(appsFile = vtsAppsFile) {
  ensureBinariesBuilt();
  const child = spawnProcess("vts", vtsBinary, [], {
    cwd: repoRoot,
    env: {
      ...process.env,
      VTS_PORT: String(vtsPort),
      VTS_APPS_FILE: appsFile,
    },
  });
  await waitForHttpOk(`http://127.0.0.1:${vtsPort}/healthz`, 30_000);
  return child;
}

export function nativeRelayAuthEnv() {
  return {
    AUTH_VTS_URL: vtsVerifyUrl,
    AUTH_RELAY_TOKEN: mintToken(relayAppId, "8760h"),
  };
}
