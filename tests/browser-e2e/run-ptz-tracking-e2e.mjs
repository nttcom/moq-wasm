#!/usr/bin/env node

import { resolve } from "node:path";

import { spawnProcess, waitForOutput } from "./process.mjs";
import { repoRoot } from "../../scripts/dev-env.mjs";
import { buildBinaries, findFreePort } from "./helpers.mjs";
import { answerQuestions, runSystemOneBotE2E } from "./system-one-bot-e2e.mjs";

async function startCamera({ botDir, moqtUrl, app, childProcesses }) {
  const cameraPort = await findFreePort();
  const camera = spawnProcess(
    "camera",
    "uv",
    [
      "run",
      "--project",
      botDir,
      "python",
      resolve(repoRoot, "tests/browser-e2e/fake-ingest-gateway-pull.py"),
      "--relay-url",
      // The camera and moq-cli connect over raw QUIC on the relay's port.
      moqtUrl.replace(/^https:/, "moqt:"),
      "--moq-cli",
      moqCliBinary,
      "--port",
      String(cameraPort),
      "--room",
      `${app.appId}/moq-ptz-tracking`,
      "--token",
      app.token,
    ],
    { cwd: repoRoot },
  );
  childProcesses.push(camera);
  await waitForOutput(camera, /video started/, "camera", 180_000);
  return { PTZ_TRACKING_E2E_CAMERA_ORIGIN: `http://127.0.0.1:${cameraPort}` };
}

const [moqCliBinary] = buildBinaries(["moq-cli"]);
runSystemOneBotE2E({
  name: "ptz-tracking",
  answer: answerQuestions,
  settings: { boxes: { 人: [[0.4, 0.4, 0.6, 0.6]] } },
  startBeforeBot: startCamera,
});
