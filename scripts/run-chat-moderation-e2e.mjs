#!/usr/bin/env node

import { createServer } from "node:http";
import { resolve } from "node:path";

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

const chatModerationPath = "/moq-wasm/examples/moq-chat-moderation/index.html";
const botDir = resolve(repoRoot, "examples/python/moq-chat-moderation");
const abusiveText = "e2e-abusive";
const childProcesses = [];

async function startFakeJev() {
  const server = createServer((request, response) => {
    let body = "";
    request.on("data", (chunk) => {
      body += chunk;
    });
    request.on("end", () => {
      const { state } = JSON.parse(body);
      const noul = state.includes(abusiveText) ? 0.95 : 0.02;
      response.setHeader("content-type", "application/json");
      response.end(JSON.stringify({ answers: { abusive: { noul } } }));
    });
  });
  await new Promise((resolvePromise) =>
    server.listen(0, "127.0.0.1", resolvePromise),
  );
  return server;
}

async function main() {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();
  const moqtUrl = getDefaultMoqtUrl();
  const fakeJev = await startFakeJev();
  const fakeJevUrl = `http://127.0.0.1:${fakeJev.address().port}/v1/systemone`;

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
    fakeJev.close();
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
      waitForHttpOk(`${baseUrl}${chatModerationPath}`, 120_000),
    ]);

    const bot = spawnProcess(
      "bot",
      "uv",
      [
        "run",
        "python",
        "-m",
        "moq_chat_moderation.bot",
        "--relay-url",
        moqtUrl,
        "--insecure",
        "--jev-url",
        fakeJevUrl,
      ],
      { cwd: botDir, env: { ...process.env, JEV_API_KEY: "e2e" } },
    );
    childProcesses.push(bot);
    await waitForOutput(bot, /waiting for peer broadcast/, "bot", 180_000);

    await runCommand(
      resolveCommandName("npm"),
      ["run", "e2e:chat-moderation"],
      {
        cwd: jsDir,
        env: {
          ...process.env,
          MEDIA_E2E_BASE_URL: baseUrl,
          CHAT_MODERATION_E2E_MOQT_URL: moqtUrl,
          CHAT_MODERATION_E2E_ABUSIVE_TEXT: abusiveText,
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
