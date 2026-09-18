#!/usr/bin/env node
// Orchestration runner for meeting E2E tests.
// Starts redis + vts + relay-a (4433) + relay-b (4434) via docker compose,
// launches vite preview, then runs the Playwright meeting-e2e spec with an
// app-scoped JWT minted from the development ledger.
//
// If relay-a and relay-b containers serving ports 4433/4434 are already running
// (e.g. from a prior test run or a sibling compose project), the runner re-uses
// them and skips docker compose up to avoid port-allocation conflicts.

import { execFileSync } from "node:child_process";
import {
  registerSignalHandlers,
  runCommand,
  spawnProcess,
  terminateProcess,
  waitForOutput,
} from "./browser-e2e-process.mjs";
import {
  assertPathExists,
  certPath,
  ensureLinuxEnvironment,
  keyPath,
  getDefaultBaseUrl,
  getDefaultWebPort,
  getErrorMessage,
  jsDir,
  repoRoot,
  resolveCommandName,
  waitForHttpOk,
} from "./media-e2e-helpers.mjs";
import { resolveLocalRelayUrl } from "./resolve-local-relay-url.mjs";
import { experimentAppId, mintToken } from "./vts-dev.mjs";

const meetingIndexPath = "/moq-wasm/examples/meeting/index.html";
const defaultRelayAUrl = "https://127.0.0.1:4433";
const defaultRelayBUrl = "https://127.0.0.1:4434";
const composeServices = ["redis", "vts", "relay-a", "relay-b"];

const childProcesses = [];
const playwrightArgs = process.argv.slice(2);
let ownedDockerServices = false;

async function main() {
  ensureLinuxEnvironment();
  assertPathExists(
    certPath,
    "TLS certificate",
    "Run node scripts/setup-media-e2e.mjs first.",
  );
  assertPathExists(
    keyPath,
    "TLS private key",
    "Run node scripts/setup-media-e2e.mjs first.",
  );
  assertPathExists(
    `${jsDir}/node_modules`,
    "examples/browser/node_modules",
    "Run npm install in examples/browser first.",
  );
  assertPathExists(
    `${jsDir}/pkg/moqt_client_wasm.js`,
    "bindings/wasm build output",
    "Run node scripts/setup-media-e2e.mjs first.",
  );

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
    if (ownedDockerServices) {
      await runCommand(
        resolveCommandName("docker"),
        ["compose", "stop", ...composeServices],
        { cwd: repoRoot },
      ).catch(() => {});
    }
  };

  registerSignalHandlers(cleanup);

  try {
    const relaysAlreadyRunning = areRelayPortsAlreadyBound();
    if (relaysAlreadyRunning) {
      console.error(
        "[setup] Relay ports 4433/4434 already occupied — reusing existing relay containers.",
      );
    } else {
      if (relayImageExists()) {
        console.error(
          "[setup] Reusing existing moqt-relay:local image (skipping build).",
        );
      } else {
        console.error("[setup] Building relay docker image...");
        await runCommand(
          resolveCommandName("docker"),
          ["compose", "build", "relay-common"],
          { cwd: repoRoot },
        );
      }

      console.error("[setup] Building vts docker image...");
      await runCommand(
        resolveCommandName("docker"),
        ["compose", "build", "vts"],
        { cwd: repoRoot },
      );
      console.error(
        `[setup] Starting ${composeServices.join(", ")} via docker compose...`,
      );
      await runCommand(
        resolveCommandName("docker"),
        ["compose", "up", "-d", "--wait", ...composeServices],
        { cwd: repoRoot },
      );
      ownedDockerServices = true;
    }

    // When reusing existing containers the relays are already bound to their
    // ports, so they are ready by definition. For newly-started containers
    // we follow the compose logs until both emit "Relay server started".
    const relayReadyPromise = relaysAlreadyRunning
      ? Promise.resolve()
      : waitForRelaysStarted();

    const vite = spawnProcess(
      "vite",
      resolveCommandName("npm"),
      [
        "exec",
        "vite",
        "--",
        "--host",
        "127.0.0.1",
        "--port",
        String(webPort),
        "--strictPort",
      ],
      { cwd: jsDir },
    );
    childProcesses.push(vite);

    await Promise.all([
      relayReadyPromise,
      waitForHttpOk(`${baseUrl}${meetingIndexPath}`, 120_000),
    ]);

    const relayAUrl = getMeetingRelayUrl(
      "MEETING_E2E_RELAY_A_URL",
      defaultRelayAUrl,
    );
    const relayBUrl = getMeetingRelayUrl(
      "MEETING_E2E_RELAY_B_URL",
      defaultRelayBUrl,
    );
    console.error(
      `[setup] Using meeting relay URLs: ${relayAUrl}, ${relayBUrl}`,
    );

    await runCommand(
      resolveCommandName("npm"),
      playwrightArgs.length > 0
        ? ["run", "e2e:meeting", "--", ...playwrightArgs]
        : ["run", "e2e:meeting"],
      {
        cwd: jsDir,
        env: {
          ...process.env,
          MEDIA_E2E_BASE_URL: baseUrl,
          MEETING_E2E_RELAY_A_URL: relayAUrl,
          MEETING_E2E_RELAY_B_URL: relayBUrl,
          MEETING_E2E_JWT: mintToken(experimentAppId, "12h"),
        },
      },
    );
  } finally {
    await cleanup();
  }
}

function getMeetingRelayUrl(envName, defaultUrl) {
  const url =
    process.env[envName] ?? resolveLocalRelayUrl(defaultUrl).toString();
  return url.replace(/\/$/, "");
}

function relayImageExists() {
  try {
    execFileSync("docker", ["image", "inspect", "moqt-relay:local"], {
      stdio: "ignore",
    });
    return true;
  } catch (_error) {
    return false;
  }
}

// Uses docker ps port output; not a UDP-level probe.
function areRelayPortsAlreadyBound() {
  try {
    const out = execFileSync("docker", ["ps", "--format", "{{.Ports}}"], {
      encoding: "utf8",
    });
    const has4433 = out.includes(":4433->443/udp");
    const has4434 = out.includes(":4434->443/udp");
    return has4433 && has4434;
  } catch (_error) {
    return false;
  }
}

async function waitForRelaysStarted() {
  const logs = spawnProcess(
    "relay-logs",
    resolveCommandName("docker"),
    ["compose", "logs", "--follow", "--no-color", "relay-a", "relay-b"],
    { cwd: repoRoot },
  );
  childProcesses.push(logs);
  await waitForOutput(
    logs,
    /Relay server started[\s\S]*Relay server started/,
    "relays",
    180_000,
  );
  await terminateProcess(logs);
}

main().catch((error) => {
  console.error(getErrorMessage(error));
  process.exitCode = 1;
});
