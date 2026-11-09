#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { resolve } from "node:path";

import { spawnProcess, waitForOutput } from "./process.mjs";
import { repoRoot } from "../../scripts/dev-env.mjs";
import { findFreePort } from "./helpers.mjs";
import { runSystemOneBotE2E } from "./system-one-bot-e2e.mjs";

const moqCliBinary = resolve(repoRoot, "target/debug/moq-cli");

/// Answers the bot's boxes questions, reading each target from its `Every <target>.` instructions.
/// `PUT /boxes` with `{"<target>": [[left, top, right, bottom], ...]}` (fractions of the picture) sets
/// each target's boxes; a target left out has none.
function answerQuestions({ questions }, { boxes }) {
  return Object.fromEntries(
    Object.entries(questions).map(([id, question]) => {
      const target = question.instructions.replace(/^Every (.*)\.$/, "$1");
      const found = (boxes[target] ?? []).map(([left, top, right, bottom]) => ({
        box: { left, top, right, bottom },
        found: 0.9,
      }));
      return [id, { type: "boxes", boxes: found, count: found.length }];
    }),
  );
}

async function startCamera({ botDir, moqtUrl, childProcesses }) {
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
  return { PTZ_TRACKING_E2E_CAMERA_ORIGIN: `http://127.0.0.1:${cameraPort}` };
}

execFileSync("cargo", ["build", "-p", "moq-cli"], {
  cwd: repoRoot,
  stdio: "inherit",
});
runSystemOneBotE2E({
  name: "ptz-tracking",
  answer: answerQuestions,
  settings: { boxes: { 人: [[0.4, 0.4, 0.6, 0.6]] } },
  startBeforeBot: startCamera,
});
