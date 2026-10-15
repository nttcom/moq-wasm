#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

// Two relays sharing a Redis route registry, live-ingest publishing an RTMP
// stream to relay A and the live viewer watching it there. The spec stops
// relay A with SIGINT, which makes it send GOAWAY naming relay B.

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
  buildLiveIngestFfmpegArgs,
  getDefaultBaseUrl,
  getDefaultWebPort,
  jsDir,
  liveViewerPath,
  waitForHttpOk,
} from "./helpers.mjs";
import { nativeRelayAuthEnv, startVts } from "./vts-dev.mjs";

const redisPort = "6399";
const redisContainer = `goaway-e2e-redis-${process.pid}`;
/// A bridged container adds a veth interface to a Linux host, and Chromium
/// aborts in-flight requests with ERR_NETWORK_CHANGED when it appears.
/// Docker Desktop keeps its interfaces in its VM.
const redisNetworkArgs =
  process.platform === "linux"
    ? ["--network", "host"]
    : ["-p", `127.0.0.1:${redisPort}:6379`];
const redisPortArgs =
  process.platform === "linux"
    ? ["--port", redisPort, "--bind", "127.0.0.1"]
    : [];
const rtmpAddress = "127.0.0.1:1935";
const srtAddress = "127.0.0.1:9000";
const relays = [
  { id: "relay-a", port: 4433, innerPort: 4443 },
  { id: "relay-b", port: 4434, innerPort: 4444 },
];
const childProcesses = [];

function spawnRelay({ id, port, innerPort }) {
  return spawnProcess(id, `${repoRoot}/target/debug/relay`, [], {
    cwd: repoRoot,
    env: {
      ...process.env,
      ...nativeRelayAuthEnv(),
      OTEL_SDK_DISABLED: "true",
      RELAY_ID: id,
      RELAY_PORT: String(port),
      RELAY_INNER_PORT: String(innerPort),
      RELAY_ADVERTISE_HOST: "127.0.0.1",
      RELAY_PUBLIC_HOST: `127.0.0.1:${port}`,
      REDIS_URL: `redis://127.0.0.1:${redisPort}`,
    },
  });
}

/// The digest-pinned Redis image of docker-compose.yml.
function composeRedisImage() {
  const compose = readFileSync(`${repoRoot}/docker-compose.yml`, "utf8");
  const image = compose.match(/^ {2}redis:\n {4}image: (\S+)$/m)?.[1];
  if (!image) {
    throw new Error("docker-compose.yml has no redis image");
  }
  return image;
}

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();
  const namespace = `anon/goaway/${Date.now()}`;

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
    try {
      execFileSync("docker", ["rm", "-f", redisContainer], { stdio: "ignore" });
    } catch {
      // Already removed with the stopped container.
    }
  };
  registerSignalHandlers(cleanup);

  try {
    // One build per package, as CI builds them: a combined build unifies
    // features differently and recompiles the dependencies.
    for (const pkg of ["relay", "live-ingest"]) {
      await runCommand("cargo", ["build", "-p", pkg], {
        cwd: repoRoot,
        env: process.env,
      });
    }
    // A Redis of its own: relays registered in a shared one would be
    // candidates for the GOAWAY target.
    const redis = spawnProcess(
      "redis",
      resolveCommandName("docker"),
      [
        "run",
        "--rm",
        "--name",
        redisContainer,
        ...redisNetworkArgs,
        composeRedisImage(),
        "redis-server",
        ...redisPortArgs,
        "--save",
        "",
        "--appendonly",
        "no",
      ],
      { cwd: repoRoot },
    );
    childProcesses.push(redis);
    await waitForOutput(redis, /Ready to accept connections/, "redis", 120_000);
    const vts = await startVts();
    childProcesses.push(vts);
    const [relayA, relayB] = relays.map(spawnRelay);
    const vite = spawnViteServer(webPort);
    childProcesses.push(relayA, relayB, vite);
    await Promise.all([
      waitForOutput(relayA, /Relay server started/, "relay-a", 60_000),
      waitForOutput(relayB, /Relay server started/, "relay-b", 60_000),
      waitForHttpOk(`${baseUrl}${liveViewerPath}`, 120_000),
    ]);

    const bridge = spawnProcess(
      "live-ingest",
      `${repoRoot}/target/debug/live-ingest`,
      [
        "--rtmp-addr",
        rtmpAddress,
        "--srt-addr",
        srtAddress,
        "--moqt-url",
        `moqt://127.0.0.1:${relays[0].port}`,
        "--insecure-skip-tls-verify",
      ],
      { cwd: repoRoot },
    );
    childProcesses.push(bridge);
    await waitForOutput(bridge, /RTMP listener started/, "live-ingest", 60_000);
    const ffmpeg = spawnProcess(
      "ffmpeg",
      "ffmpeg",
      buildLiveIngestFfmpegArgs(rtmpAddress, namespace),
      {
        cwd: repoRoot,
      },
    );
    childProcesses.push(ffmpeg);
    await waitForOutput(
      bridge,
      /namespace published/,
      "live-ingest publish",
      60_000,
    );

    await runCommand(resolveCommandName("npm"), ["run", "e2e:goaway"], {
      cwd: jsDir,
      env: {
        ...process.env,
        MEDIA_E2E_BASE_URL: baseUrl,
        MEDIA_E2E_MOQT_URL: `https://127.0.0.1:${relays[0].port}`,
        LIVE_VIEWER_E2E_NAMESPACE: namespace,
        GOAWAY_E2E_RELAY_A_PID: String(relayA.pid),
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
