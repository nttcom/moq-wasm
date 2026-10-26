#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

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
  waitForHttpOk,
} from "./helpers.mjs";
import { nativeRelayAuthEnv, startVts } from "./vts-dev.mjs";

const botDir = resolve(repoRoot, "examples/python/moq-meeting-transcriber");
const meetingPath = "/moq-wasm/examples/meeting/index.html";
const speechText =
  "こんにちは。これは会議の書き起こしのテストです。今日はいい天気ですね。";
// The smallest multilingual Whisper model, so the run downloads and loads little.
const whisperModel = "tiny";
const childProcesses = [];

/// Speech for the fake microphone, which Chrome loops.
function synthesizeSpeech() {
  const file = join(mkdtempSync(join(tmpdir(), "meeting-transcription-e2e-")), "speech.wav");
  if (process.platform === "darwin") {
    execFileSync("say", ["-v", "Kyoko", "-o", file, "--data-format=LEI16@48000", speechText]);
  } else {
    // Open JTalk reads kanji; espeak-ng's ja voice spells each one out as "Chinese letter".
    execFileSync(
      "open_jtalk",
      [
        "-x",
        "/var/lib/mecab/dic/open-jtalk/naist-jdic",
        "-m",
        "/usr/share/hts-voice/nitech-jp-atr503-m001/nitech_jp_atr503_m001.htsvoice",
        "-ow",
        file,
      ],
      { input: speechText },
    );
  }
  return file;
}

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();
  const moqtUrl = getDefaultMoqtUrl();
  const speech = synthesizeSpeech();

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
      waitForHttpOk(`${baseUrl}${meetingPath}`, 120_000),
    ]);

    const botPort = await findFreePort();
    // The spec serves the fake ASR service on this port (examples/browser/tests/fake-asr.ts).
    const asrPort = await findFreePort();
    const bot = spawnProcess(
      "bot",
      "uv",
      [
        "run",
        "python",
        "-m",
        "moq_meeting_transcriber.bot",
        "--port",
        String(botPort),
        "--relay-url",
        moqtUrl,
        "--insecure",
        "--model",
        whisperModel,
        "--asr",
        `granite-speech-5-2b=http://127.0.0.1:${asrPort}`,
      ],
      { cwd: botDir },
    );
    childProcesses.push(bot);
    // The first run builds the moqt bindings and downloads the model.
    await waitForOutput(bot, /serving rooms on/, "bot", 900_000);

    await runCommand(
      resolveCommandName("npm"),
      ["run", "e2e:meeting-transcription"],
      {
        cwd: jsDir,
        env: {
          ...process.env,
          MEDIA_E2E_BASE_URL: baseUrl,
          MEETING_E2E_RELAY_A_URL: moqtUrl,
          MEETING_E2E_RELAY_B_URL: moqtUrl,
          MEETING_TRANSCRIPTION_E2E_BOT_URL: `http://127.0.0.1:${botPort}`,
          MEETING_TRANSCRIPTION_E2E_ASR_PORT: String(asrPort),
          E2E_FAKE_AUDIO_FILE: speech,
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
