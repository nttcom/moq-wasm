#!/usr/bin/env node

import { once } from "node:events";
import { createServer } from "node:http";
import { resolve } from "node:path";
import { text } from "node:stream/consumers";

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
  cameraDetectionPath,
  ensureLinuxEnvironment,
  getDefaultBaseUrl,
  getDefaultMoqtUrl,
  getDefaultWebPort,
  getErrorMessage,
  jsDir,
  repoRoot,
  resolveCommandName,
  waitForHttpOk,
} from "./media-e2e-helpers.mjs";
import { nativeRelayAuthEnv, startVts } from "./vts-dev.mjs";

const botDir = resolve(repoRoot, "examples/python/moq-camera-detection");
const childProcesses = [];

async function startFakeDjevVision() {
  let answer = "1";
  const server = createServer(async (request, response) => {
    if (request.method === "PUT" && request.url === "/answer") {
      answer = await text(request);
      response.statusCode = 204;
      response.end();
      return;
    }
    response.setHeader("content-type", "application/json");
    response.end(
      JSON.stringify({
        choices: [{ message: { content: `thought\n${answer}` } }],
      }),
    );
  });
  await once(server.listen(0, "127.0.0.1"), "listening");
  return server;
}

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();
  const moqtUrl = getDefaultMoqtUrl();
  const fakeDjev = await startFakeDjevVision();
  const fakeDjevOrigin = `http://127.0.0.1:${fakeDjev.address().port}`;

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
    fakeDjev.close();
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
      waitForHttpOk(`${baseUrl}${cameraDetectionPath}`, 120_000),
    ]);

    const bot = spawnProcess(
      "bot",
      "uv",
      [
        "run",
        "python",
        "-m",
        "moq_camera_detection.bot",
        "--relay-url",
        moqtUrl,
        "--insecure",
        "--djev-url",
        `${fakeDjevOrigin}/v1/chat/completions`,
      ],
      { cwd: botDir },
    );
    childProcesses.push(bot);
    await waitForOutput(bot, /pipeline is now ready/, "bot", 180_000);

    await runCommand(
      resolveCommandName("npm"),
      ["run", "e2e:camera-detection"],
      {
        cwd: jsDir,
        env: {
          ...process.env,
          MEDIA_E2E_BASE_URL: baseUrl,
          CAMERA_DETECTION_E2E_MOQT_URL: moqtUrl,
          CAMERA_DETECTION_E2E_ANSWER_URL: `${fakeDjevOrigin}/answer`,
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
