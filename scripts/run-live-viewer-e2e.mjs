#!/usr/bin/env node

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
  getDefaultBaseUrl,
  getDefaultMoqtUrl,
  getDefaultWebPort,
  getErrorMessage,
  jsDir,
  keyPath,
  liveViewerPath,
  repoRoot,
  resolveCommandName,
  waitForHttpOk,
} from "./media-e2e-helpers.mjs";

const rtmpAddress = process.env.LIVE_VIEWER_E2E_RTMP_ADDR ?? "127.0.0.1:1935";
const srtAddress = process.env.LIVE_VIEWER_E2E_SRT_ADDR ?? "127.0.0.1:9000";
const childProcesses = [];

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
    "Run node scripts/setup-media-e2e.mjs first.",
  );
  assertPathExists(
    `${jsDir}/pkg/moqt_client_wasm.js`,
    "bindings/wasm build output",
    "Run node scripts/setup-media-e2e.mjs first.",
  );

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();
  const moqtUrl = getDefaultMoqtUrl();
  const namespace = process.env.LIVE_VIEWER_E2E_NAMESPACE ?? "live/e2e";

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
  };

  registerSignalHandlers(cleanup);

  try {
    const server = spawnProcess("server", "cargo", ["run", "-p", "relay"], {
      cwd: repoRoot,
      env: { ...process.env, AUTH_DISABLED: "true" },
    });
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
        "moqt-bridge-live-ingest",
        "--",
        "--rtmp-addr",
        rtmpAddress,
        "--srt-addr",
        srtAddress,
        "--moqt-url",
        moqtUrl,
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
      buildFfmpegArgs(namespace),
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

    await runCommand(resolveCommandName("npm"), ["run", "e2e:live-viewer"], {
      cwd: jsDir,
      env: {
        ...process.env,
        MEDIA_E2E_BASE_URL: baseUrl,
        MEDIA_E2E_MOQT_URL: moqtUrl,
        LIVE_VIEWER_E2E_NAMESPACE: namespace,
      },
    });
  } finally {
    await cleanup();
  }
}

function buildFfmpegArgs(namespace) {
  return [
    "-hide_banner",
    "-loglevel",
    "warning",
    "-re",
    "-f",
    "lavfi",
    "-i",
    "testsrc=size=1280x720:rate=30",
    "-f",
    "lavfi",
    "-i",
    "sine=frequency=1000:sample_rate=48000",
    "-c:v",
    "libx264",
    "-preset",
    "veryfast",
    "-profile:v",
    "baseline",
    "-pix_fmt",
    "yuv420p",
    "-g",
    "60",
    "-sc_threshold",
    "0",
    "-c:a",
    "aac",
    "-ar",
    "48000",
    "-ac",
    "2",
    "-t",
    "300",
    "-f",
    "flv",
    `rtmp://${rtmpAddress}/${namespace}/stream`,
  ];
}

main().catch((error) => {
  console.error(getErrorMessage(error));
  process.exitCode = 1;
});
