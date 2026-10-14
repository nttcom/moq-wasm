#!/usr/bin/env node

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
  getDefaultMoqtUrl,
  getDefaultWebPort,
  jsDir,
  liveViewerPath,
  waitForHttpOk,
} from "./helpers.mjs";
import { nativeRelayAuthEnv, startVts } from "./vts-dev.mjs";

const rtmpAddress = process.env.LIVE_VIEWER_E2E_RTMP_ADDR ?? "127.0.0.1:1935";
const srtAddress = process.env.LIVE_VIEWER_E2E_SRT_ADDR ?? "127.0.0.1:9000";
const childProcesses = [];

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();
  const moqtUrl = getDefaultMoqtUrl();
  const namespace = process.env.LIVE_VIEWER_E2E_NAMESPACE ?? "anon/live/e2e";

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
  };

  registerSignalHandlers(cleanup);

  try {
    const vts = await startVts();
    childProcesses.push(vts);
    const server = spawnProcess("server", "cargo", ["run", "-p", "relay"], {
      cwd: repoRoot,
      env: { ...process.env, ...nativeRelayAuthEnv() },
    });
    const vite = spawnViteServer(webPort);

    childProcesses.push(server, vite);

    await Promise.all([
      waitForOutput(server, /Relay server started/, "relay", 180_000),
      waitForHttpOk(`${baseUrl}${liveViewerPath}`, 120_000),
    ]);

    const bridge = spawnProcess(
      "live-ingest",
      "cargo",
      [
        "run",
        "-p",
        "live-ingest",
        "--",
        "--rtmp-addr",
        rtmpAddress,
        "--srt-addr",
        srtAddress,
        "--moqt-url",
        moqtUrl,
        "--insecure-skip-tls-verify",
        "--transcode",
      ],
      { cwd: repoRoot },
    );
    childProcesses.push(bridge);
    await waitForOutput(
      bridge,
      /RTMP listener started/,
      "live-ingest",
      180_000,
    );

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
      /transcoding renditions started/,
      "live-ingest renditions",
      120_000,
    );

    await runCommand(
      resolveCommandName("npm"),
      ["run", "e2e:live-viewer", "--", "tests/live-viewer-mp4-e2e.spec.ts"],
      {
        cwd: jsDir,
        env: {
          ...process.env,
          MEDIA_E2E_BASE_URL: baseUrl,
          MEDIA_E2E_MOQT_URL: moqtUrl,
          LIVE_VIEWER_E2E_NAMESPACE: namespace,
        },
      },
    );
  } finally {
    await cleanup();
  }
}

main().catch((error) => {
  console.error(getErrorMessage(error));
  process.exitCode = 1;
});
