#!/usr/bin/env node

import { createWriteStream, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { runCommand, spawnProcess, waitForOutput } from "./process.mjs";
import {
  ensureLinuxEnvironment,
  getErrorMessage,
  repoRoot,
  resolveCommandName,
} from "../../scripts/dev-env.mjs";
import {
  assertE2EPrerequisites,
  buildBinaries,
  buildLiveIngestFfmpegArgs,
  getDefaultBaseUrl,
  getDefaultMoqtUrl,
  jsDir,
  liveViewerPath,
} from "./helpers.mjs";
import { startRelayAndPage } from "./relay-dev.mjs";

const rtmpAddress = process.env.LIVE_VIEWER_E2E_RTMP_ADDR ?? "127.0.0.1:1935";
const srtAddress = process.env.LIVE_VIEWER_E2E_SRT_ADDR ?? "127.0.0.1:9000";
const ingest = process.env.LIVE_VIEWER_E2E_INGEST ?? "rtmp";
const ingestAddress = ingest === "srt" ? srtAddress : rtmpAddress;
const transcode = process.env.LIVE_VIEWER_E2E_TRANSCODE !== "0";
const specs =
  process.argv.length > 2
    ? process.argv.slice(2)
    : [
        "tests/live-viewer-e2e.spec.ts",
        "tests/live-viewer-mp4-e2e.spec.ts",
        "tests/recorder-serve-e2e.spec.ts",
        "tests/live-viewer-delivery-e2e.spec.ts",
        "tests/embed-e2e.spec.ts",
      ];

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const baseUrl = getDefaultBaseUrl();
  const moqtUrl = getDefaultMoqtUrl();
  const namespace = process.env.LIVE_VIEWER_E2E_NAMESPACE ?? "anon/live/e2e";

  // The recorder specs run target/debug/recorder and target/debug/bot-dispatcher.
  const [liveIngestBinary] = buildBinaries([
    "live-ingest",
    "recorder",
    "bot-dispatcher",
    "relay",
  ]);
  const { childProcesses, cleanup, ready } = startRelayAndPage(liveViewerPath);

  try {
    await ready;

    // live-viewer-delivery-e2e.spec.ts compares what the viewer received
    // against the samples this log says the bridge published.
    const bridgeLog = join(
      mkdtempSync(join(tmpdir(), "live-viewer-e2e-")),
      "live-ingest.log",
    );
    const bridge = spawnProcess(
      "live-ingest",
      liveIngestBinary,
      [
        "--rtmp-addr",
        rtmpAddress,
        "--srt-addr",
        srtAddress,
        "--moqt-url",
        moqtUrl,
        "--insecure-skip-tls-verify",
        ...(transcode ? ["--transcode"] : []),
      ],
      {
        cwd: repoRoot,
        env: {
          ...process.env,
          RUST_LOG: `${process.env.RUST_LOG ?? "info"},broadcast::delivery=debug`,
        },
        hiddenLines: /broadcast::delivery/,
      },
    );
    bridge.stdout.pipe(createWriteStream(bridgeLog));
    childProcesses.push(bridge);
    await waitForOutput(
      bridge,
      ingest === "srt" ? /SRT listener started/ : /RTMP listener started/,
      "live-ingest",
      180_000,
    );

    const broadcastReady = waitForOutput(
      bridge,
      transcode ? /transcoding renditions started/ : /detected video codec/,
      "live-ingest broadcast",
      120_000,
    );
    const ffmpeg = spawnProcess(
      "ffmpeg",
      "ffmpeg",
      buildLiveIngestFfmpegArgs(ingest, ingestAddress, namespace),
      {
        cwd: repoRoot,
      },
    );
    childProcesses.push(ffmpeg);
    await broadcastReady;

    await runCommand(
      resolveCommandName("npm"),
      ["exec", "--", "playwright", "test", ...specs],
      {
        cwd: jsDir,
        env: {
          ...process.env,
          MEDIA_E2E_BASE_URL: baseUrl,
          MEDIA_E2E_MOQT_URL: moqtUrl,
          LIVE_VIEWER_E2E_NAMESPACE: namespace,
          DELIVERY_BRIDGE_LOG: bridgeLog,
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
