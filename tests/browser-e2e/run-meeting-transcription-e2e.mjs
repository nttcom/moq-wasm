#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { runCommand } from "./process.mjs";
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
  jsDir,
} from "./helpers.mjs";
import { localPythonBot, startBots, temporaryAppSecret } from "./bots-dev.mjs";
import { startRelayAndPage } from "./relay-dev.mjs";

const botDir = resolve(repoRoot, "examples/python/moq-meeting-transcriber");
const meetingPath = "/moq-wasm/examples/meeting/index.html";
const speechText =
  "こんにちは。これは会議の書き起こしのテストです。今日はいい天気ですね。";
// The smallest multilingual Whisper model, so the run downloads and loads little.
const whisperModel = "tiny";

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

  const baseUrl = getDefaultBaseUrl();
  const moqtUrl = getDefaultMoqtUrl();
  const speech = synthesizeSpeech();

  process.env.VTS_TEMPORARY_APP_SECRET = temporaryAppSecret;
  // Watched from now: the relay may start while the builds below run.
  const { childProcesses, cleanup, ready } = startRelayAndPage(meetingPath, {
    timeoutMs: 600_000,
  });

  try {
    // The first run builds the moq-wasm SDK into the bot's environment.
    await runCommand(resolveCommandName("uv"), ["sync"], { cwd: botDir });
    await ready;

    // The spec serves the fake ASR service on this port (examples/browser/tests/fake-asr.ts).
    const asrPort = await findFreePort();
    const temporaryAppsUrl = await startBots(
      {
        transcriber: localPythonBot(botDir, "moq_meeting_transcriber.bot", [
          "--relay-url",
          moqtUrl,
          "--insecure",
          "--model",
          whisperModel,
          "--asr",
          `granite-speech-5-2b=http://127.0.0.1:${asrPort}`,
        ]),
      },
      childProcesses,
    );

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
          MEETING_TRANSCRIPTION_E2E_TEMPORARY_APPS_URL: temporaryAppsUrl,
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
