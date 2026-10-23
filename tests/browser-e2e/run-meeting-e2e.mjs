#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import {
  registerSignalHandlers,
  runCommand,
  spawnProcess,
  spawnViteServer,
  terminateProcess,
  waitForOutput,
} from "./process.mjs";
import {
  ensureLinuxEnvironment,
  getErrorMessage,
  repoRoot,
  resolveCommandName,
} from "../../scripts/dev-env.mjs";
import {
  assertE2EPrerequisites,
  getDefaultBaseUrl,
  getDefaultWebPort,
  jsDir,
  waitForHttpOk,
} from "./helpers.mjs";
import { resolveLocalRelayUrl } from "../../scripts/resolve-local-relay-url.mjs";
import { experimentAppId, mintToken, startVts, vtsPort } from "./vts-dev.mjs";

const meetingIndexPath = "/moq-wasm/examples/meeting/index.html";
const composeServices = ["redis", "relay-a", "relay-b"];
// Reuses the vts binary that token minting already builds, instead of the compose vts service.
const containerVtsUrl = `http://host.docker.internal:${vtsPort}/verify`;
// Over loopback a send stream close that awaits the FIN's ACK takes under 10 ms,
// inside the meeting E2E's 50 ms bound. With this delay it takes at least 60 ms.
const relayReplyDelayMs = 60;

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

      const vts = await startVts();
      childProcesses.push(vts);
      console.error(
        `[setup] Starting ${composeServices.join(", ")} via docker compose...`,
      );
      // --no-deps keeps compose from starting its own vts but also skips redis;
      // relays connect to redis only at startup, so redis comes up first.
      await dockerCompose("up", "-d", "--wait", "redis");
      await dockerCompose(
        "up",
        "-d",
        "--wait",
        "--no-deps",
        "relay-a",
        "relay-b",
      );
      ownedDockerServices = true;
      await delayRelayReplies();
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
    env: { ...process.env, AUTH_VTS_URL: containerVtsUrl },
  });
}

// The relay image is distroless, so the host's tc runs inside each relay's
// network namespace. Linux only: Docker Desktop keeps the namespace in its VM.
async function delayRelayReplies() {
  if (process.platform !== "linux") {
    console.error(
      "[setup] Running without the relay reply delay: tc netem needs Linux.",
    );
    return;
  }
  for (const service of ["relay-a", "relay-b"]) {
    const containerId = execFileSync(
      "docker",
      ["compose", "ps", "-q", service],
      { cwd: repoRoot, encoding: "utf8" },
    ).trim();
    const pid = execFileSync(
      "docker",
      ["inspect", "-f", "{{.State.Pid}}", containerId],
      { encoding: "utf8" },
    ).trim();
    await runCommand(
      "sudo",
      [
        "-n",
        "nsenter",
        "-t",
        pid,
        "-n",
        "tc",
        "qdisc",
        "add",
        "dev",
        "eth0",
        "root",
        "netem",
        "delay",
        `${relayReplyDelayMs}ms`,
      ],
      { cwd: repoRoot, env: process.env },
    );
  }
  console.error(
    `[setup] Delaying every relay reply by ${relayReplyDelayMs} ms.`,
  );
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
