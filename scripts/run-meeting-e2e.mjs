#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import {
  registerSignalHandlers,
  runCommand,
  spawnProcess,
  spawnViteServer,
  terminateProcess,
  waitForOutput,
} from "./browser-e2e-process.mjs";
import {
  assertE2EPrerequisites,
  ensureLinuxEnvironment,
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
const composeServices = ["redis", "vts", "relay-a", "relay-b"];

const childProcesses = [];
const playwrightArgs = process.argv.slice(2);
let ownedDockerServices = false;

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
    if (ownedDockerServices) {
      await dockerCompose("stop", ...composeServices).catch(() => {});
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
        await dockerCompose("build", "relay-common");
      }

      console.error("[setup] Building vts docker image...");
      await dockerCompose("build", "vts");
      console.error(
        `[setup] Starting ${composeServices.join(", ")} via docker compose...`,
      );
      await dockerCompose("up", "-d", "--wait", ...composeServices);
      ownedDockerServices = true;
    }

    const relayReadyPromise = relaysAlreadyRunning
      ? Promise.resolve()
      : waitForRelaysStarted();

    const vite = spawnViteServer(webPort);
    childProcesses.push(vite);

    await Promise.all([
      relayReadyPromise,
      waitForHttpOk(`${baseUrl}${meetingIndexPath}`, 120_000),
    ]);

    const relayAUrl = getMeetingRelayUrl("MEETING_E2E_RELAY_A_URL", 4433);
    const relayBUrl = getMeetingRelayUrl("MEETING_E2E_RELAY_B_URL", 4434);
    console.error(
      `[setup] Using meeting relay URLs: ${relayAUrl}, ${relayBUrl}`,
    );

    await runCommand(
      resolveCommandName("npm"),
      ["run", "e2e:meeting", "--", ...playwrightArgs],
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

function dockerCompose(...args) {
  return runCommand(resolveCommandName("docker"), ["compose", ...args], {
    cwd: repoRoot,
  });
}

function getMeetingRelayUrl(envName, port) {
  const url =
    process.env[envName] ??
    resolveLocalRelayUrl(`https://127.0.0.1:${port}`).toString();
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
