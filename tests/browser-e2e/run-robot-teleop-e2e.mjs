#!/usr/bin/env node

// Runs the simulated robot (examples/rust/moq-robot-sim) and the operator page
// against one relay, and drives the robot from the page: in one room alone, in
// another during an episode the policy bot (examples/rust/moq-robot-policy)
// drives, and in a third the same way while the recorder records it as a MoQ
// room recording; the spec runs the recorder and bot-dispatcher this script
// builds.

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
  execFileSync(
    "cargo",
    [
      "build",
      "-p",
      "moq-robot-sim",
      "-p",
      "moq-robot-policy",
      "-p",
      "recorder",
      "-p",
      "bot-dispatcher",
    ],
    {
      cwd: repoRoot,
      stdio: "inherit",
    },
  );

  const room =
    process.env.ROBOT_TELEOP_E2E_ROOM ?? `anon/robot-teleop-e2e/${Date.now()}`;
  const policyRoom = `${room}-policy`;
  const recordedRoom = `${room}-recorded`;
  const moqtUrl = getDefaultMoqtUrl();
  const { childProcesses, cleanup, ready } =
    startRelayAndPage(robotTeleopIndexPath);

  try {
    await ready;
    // The robot and the policy dial the relay over raw QUIC on the page's port.
    const relayUrl = moqtUrl.replace(/^https:/, "moqt:");
    const start = async (label, binary, args, ready) => {
      const child = spawnProcess(
        label,
        resolve(repoRoot, "target/debug", binary),
        ["--url", relayUrl, "--insecure", ...args],
        {
          cwd: repoRoot,
        },
      );
      childProcesses.push(child);
      await waitForOutput(child, ready, label, 60_000);
    };
    await start("robot", "moq-robot-sim", ["--room", room], /robot ready/);
    for (const [label, policyRoomName] of [
      ["", policyRoom],
      ["recorded-", recordedRoom],
    ]) {
      await start(
        `${label}policy-robot`,
        "moq-robot-sim",
        ["--room", policyRoomName, "--policy-episode", "go to the corner"],
        /robot ready/,
      );
      await start(
        `${label}policy`,
        "moq-robot-policy",
        ["--room", policyRoomName],
        /policy ready/,
      );
    }

    await runCommand(resolveCommandName("npm"), ["run", "e2e:robot-teleop"], {
      cwd: jsDir,
      env: {
        ...process.env,
        MEDIA_E2E_BASE_URL: getDefaultBaseUrl(),
        ROBOT_TELEOP_E2E_MOQT_URL: moqtUrl,
        ROBOT_TELEOP_E2E_ROOM: room,
        ROBOT_TELEOP_E2E_POLICY_ROOM: policyRoom,
        ROBOT_TELEOP_E2E_RECORDED_ROOM: recordedRoom,
        // The recorder bot dials the relay at this URL.
        MEDIA_E2E_MOQT_URL: moqtUrl,
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
