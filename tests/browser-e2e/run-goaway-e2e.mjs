#!/usr/bin/env node

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
  };
  registerSignalHandlers(cleanup);

  try {
    await runCommand("cargo", ["build", "-p", "relay", "-p", "live-ingest"], {
      cwd: repoRoot,
      env: process.env,
    });
    const redis = spawnProcess(
      "redis",
      "redis-server",
      ["--port", redisPort, "--save", "", "--appendonly", "no"],
      { cwd: repoRoot },
    );
    childProcesses.push(redis);
    await waitForOutput(redis, /Ready to accept connections/, "redis", 30_000);
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
