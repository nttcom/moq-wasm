#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

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
import { nativeRelayEnv, startVts, vtsAppsFile, vtsVerifyUrl } from "./vts-dev.mjs";

const botDir = resolve(repoRoot, "examples/python/moq-meeting-transcriber");
const meetingPath = "/moq-wasm/examples/meeting/index.html";
const speechText =
  "こんにちは。これは会議の書き起こしのテストです。今日はいい天気ですね。";
// The smallest multilingual Whisper model, so the run downloads and loads little.
const whisperModel = "tiny";
// The temporary-apps service signs the pages' tokens with this secret, the VTS verifies them with it,
// and the bot service signs the bot's token with it.
const temporaryAppSecret = "development-temporary-app-secret";
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
    process.env.VTS_TEMPORARY_APP_SECRET = temporaryAppSecret;
    const vts = await startVts();
    childProcesses.push(vts);
    const server = spawnProcess("server", "cargo", ["run", "-p", "relay"], {
      cwd: repoRoot,
      env: { ...process.env, ...nativeRelayEnv() },
    });
    const vite = spawnViteServer(webPort);
    childProcesses.push(server, vite);
    // Watched from now: the relay may start while the builds below run.
    const ready = Promise.all([
      waitForOutput(server, /Relay server started/, "relay", 600_000),
      waitForHttpOk(`${baseUrl}${meetingPath}`, 600_000),
    ]);
    // The first run builds the moq-wasm SDK into the bot's environment.
    await runCommand(resolveCommandName("uv"), ["sync"], { cwd: botDir });
    await runCommand(resolveCommandName("cargo"), ["build", "-p", "bots", "-p", "temporary-apps"], {
      cwd: repoRoot,
    });
    await ready;

    // The spec serves the fake ASR service on this port (examples/browser/tests/fake-asr.ts).
    const asrPort = await findFreePort();
    const botsPort = await findFreePort();
    const botsUrl = `http://127.0.0.1:${botsPort}`;
    const botTypes = {
      transcriber: {
        launcher: { kind: "local", program: join(botDir, ".venv", "bin", "python") },
        args: [
          "-m",
          "moq_meeting_transcriber.bot",
          "--relay-url",
          moqtUrl,
          "--insecure",
          "--model",
          whisperModel,
          "--asr",
          `granite-speech-5-2b=http://127.0.0.1:${asrPort}`,
        ],
        grace_seconds: 30,
        max_seconds: 3600,
        max_running: 4,
      },
    };
    const bots = spawnProcess("bots", resolve(repoRoot, "target/debug/bots"), [], {
      cwd: repoRoot,
      env: {
        ...process.env,
        PORT: String(botsPort),
        BOTS_PUBLIC_URL: botsUrl,
        BOTS_TYPES: JSON.stringify(botTypes),
        BOTS_STORE_URL: `${pathToFileURL(mkdtempSync(join(tmpdir(), "meeting-transcription-bots-"))).href}/`,
        BOTS_VTS_URL: vtsVerifyUrl,
        BOTS_REGISTRY_FILE: resolve(repoRoot, vtsAppsFile),
        BOTS_TEMPORARY_APP_SECRET: temporaryAppSecret,
      },
    });
    const temporaryAppsPort = await findFreePort();
    const temporaryAppsUrl = `http://127.0.0.1:${temporaryAppsPort}`;
    const temporaryApps = spawnProcess("temporary-apps", resolve(repoRoot, "target/debug/temporary-apps"), [], {
      cwd: repoRoot,
      env: {
        ...process.env,
        PORT: String(temporaryAppsPort),
        TEMPORARY_APPS_SECRET: temporaryAppSecret,
        TEMPORARY_APPS_BOTS_URL: botsUrl,
        TEMPORARY_APPS_BOT_TYPES: "transcriber",
      },
    });
    childProcesses.push(bots, temporaryApps);
    await Promise.all([
      waitForHttpOk(`${botsUrl}/healthz`, 60_000),
      waitForHttpOk(`${temporaryAppsUrl}/healthz`, 60_000),
    ]);

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
