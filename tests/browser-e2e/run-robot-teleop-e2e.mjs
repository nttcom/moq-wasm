#!/usr/bin/env node

// Runs the simulated robot (examples/rust/moq-robot-sim) and the operator page
// against one relay, and drives the robot from the page.

import { execFileSync } from "node:child_process";
import { resolve } from "node:path";

import { runCommand, spawnProcess, waitForOutput } from "./process.mjs";
import {
  ensureLinuxEnvironment,
  getErrorMessage,
  repoRoot,
  resolveCommandName,
} from "../../scripts/dev-env.mjs";
import {
  assertE2EPrerequisites,
  getDefaultBaseUrl,
  getDefaultMoqtUrl,
  jsDir,
  robotTeleopIndexPath,
} from "./helpers.mjs";
import { startRelayAndPage } from "./relay-dev.mjs";

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();
  execFileSync("cargo", ["build", "-p", "moq-robot-sim"], { cwd: repoRoot, stdio: "inherit" });

  const room = process.env.ROBOT_TELEOP_E2E_ROOM ?? `anon/robot-teleop-e2e/${Date.now()}`;
  const moqtUrl = getDefaultMoqtUrl();
  const { childProcesses, cleanup, ready } = startRelayAndPage(robotTeleopIndexPath);

  try {
    await ready;
    const robot = spawnProcess(
      "robot",
      resolve(repoRoot, "target/debug/moq-robot-sim"),
      // The robot dials the relay over raw QUIC on the page's port.
      ["--url", moqtUrl.replace(/^https:/, "moqt:"), "--insecure", "--room", room],
      { cwd: repoRoot },
    );
    childProcesses.push(robot);
    await waitForOutput(robot, /robot ready/, "robot", 60_000);

    await runCommand(resolveCommandName("npm"), ["run", "e2e:robot-teleop"], {
      cwd: jsDir,
      env: {
        ...process.env,
        MEDIA_E2E_BASE_URL: getDefaultBaseUrl(),
        ROBOT_TELEOP_E2E_MOQT_URL: moqtUrl,
        ROBOT_TELEOP_E2E_ROOM: room,
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
