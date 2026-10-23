#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:http";
import { resolve } from "node:path";
import { text } from "node:stream/consumers";
import { setTimeout } from "node:timers/promises";

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
  findFreePort,
  getDefaultBaseUrl,
  getDefaultMoqtUrl,
  getDefaultWebPort,
  jsDir,
  ptzTrackingPath,
  waitForHttpOk,
} from "./helpers.mjs";
import { nativeRelayAuthEnv, startVts } from "./vts-dev.mjs";

const botDir = resolve(repoRoot, "examples/python/moq-ptz-tracking");
const moqCliBinary = resolve(repoRoot, "target/debug/moq-cli");
const childProcesses = [];

/// Answers the bot's two d3-flash questions: whether the target is visible, and which cell of
/// the 5x5 grid holds it. `PUT /position` with "x,y" (fractions of the picture) or "not visible"
/// sets the answer; `PUT /delay` delays every answer by the given milliseconds.
async function startFakeD3Flash() {
  let position = "0.5,0.5";
  let delayMs = 0;
  const server = createServer(async (request, response) => {
    if (request.method === "PUT" && request.url === "/delay") {
      delayMs = Number(await text(request));
      response.statusCode = 204;
      response.end();
      return;
    }
    if (request.method === "PUT" && request.url === "/position") {
      position = await text(request);
      response.statusCode = 204;
      response.end();
      return;
    }
    const { questions } = JSON.parse(await text(request));
    await setTimeout(delayMs);
    response.setHeader("content-type", "application/json");
    response.end(
      JSON.stringify({ answers: answerQuestions(questions, position) }),
    );
  });
  await once(server.listen(0, "127.0.0.1"), "listening");
  return server;
}

function answerQuestions(questions, position) {
  const visible = position !== "not visible";
  const cell = visible ? cellOf(position) : undefined;
  return Object.fromEntries(
    Object.entries(questions).map(([id, question]) => [
      id,
      question.type === "noul"
        ? { noul: visible ? 0.9 : 0.1 }
        : {
            probabilities: Object.fromEntries(
              Object.keys(question.criteria).map((key) => [
                key,
                key === cell ? 1 : 0,
              ]),
            ),
          },
    ]),
  );
}

function cellOf(position) {
  const [x, y] = position.split(",").map(Number);
  const index = (fraction) => Math.min(4, Math.floor(fraction * 5));
  return `r${index(y)}c${index(x)}`;
}

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();
  execFileSync("cargo", ["build", "-p", "moq-cli"], {
    cwd: repoRoot,
    stdio: "inherit",
  });

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();
  const moqtUrl = getDefaultMoqtUrl();
  const fakeD3Flash = await startFakeD3Flash();
  const fakeD3FlashOrigin = `http://127.0.0.1:${fakeD3Flash.address().port}`;

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
    fakeD3Flash.close();
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
      waitForHttpOk(`${baseUrl}${ptzTrackingPath}`, 120_000),
    ]);

    const cameraPort = await findFreePort();
    const camera = spawnProcess(
      "camera",
      "uv",
      [
        "run",
        "--project",
        botDir,
        "python",
        resolve(repoRoot, "tests/browser-e2e/fake-onvif-ingest.py"),
        "--relay-url",
        // The camera and moq-cli connect over raw QUIC on the relay's port.
        moqtUrl.replace(/^https:/, "moqt:"),
        "--moq-cli",
        moqCliBinary,
        "--port",
        String(cameraPort),
      ],
      { cwd: repoRoot },
    );
    childProcesses.push(camera);
    await waitForOutput(camera, /codec resolved/, "camera", 180_000);

    const botPort = await findFreePort();
    const bot = spawnProcess(
      "bot",
      "uv",
      [
        "run",
        "python",
        "-m",
        "moq_ptz_tracking.bot",
        "--port",
        String(botPort),
        "--relay-url",
        moqtUrl,
        "--insecure",
        "--d3-flash-url",
        `${fakeD3FlashOrigin}/v1/systemone`,
      ],
      { cwd: botDir },
    );
    childProcesses.push(bot);
    await waitForOutput(bot, /serving rooms on/, "bot", 180_000);

    await runCommand(resolveCommandName("npm"), ["run", "e2e:ptz-tracking"], {
      cwd: jsDir,
      env: {
        ...process.env,
        MEDIA_E2E_BASE_URL: baseUrl,
        PTZ_TRACKING_E2E_MOQT_URL: moqtUrl,
        PTZ_TRACKING_E2E_BOT_URL: `http://127.0.0.1:${botPort}`,
        PTZ_TRACKING_E2E_D3_FLASH_ORIGIN: fakeD3FlashOrigin,
        PTZ_TRACKING_E2E_CAMERA_ORIGIN: `http://127.0.0.1:${cameraPort}`,
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
