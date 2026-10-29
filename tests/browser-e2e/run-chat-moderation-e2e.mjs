#!/usr/bin/env node

import { once } from "node:events";
import { createServer } from "node:http";
import { resolve } from "node:path";
import { json, text } from "node:stream/consumers";
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
  chatModerationPath,
  getDefaultBaseUrl,
  getDefaultMoqtUrl,
  getDefaultWebPort,
  jsDir,
  waitForHttpOk,
} from "./helpers.mjs";
import { nativeRelayAuthEnv, startVts } from "./vts-dev.mjs";

const botDir = resolve(repoRoot, "examples/python/moq-chat-moderation");
const childProcesses = [];

function labelNoul(state, question) {
  const [, name] = question.instructions.match(/「(.+?)」/) ?? [];
  return name !== undefined && state.includes(`e2e-${name}`) ? 0.95 : 0.02;
}

async function startFakeSystemOne() {
  let delayMs = 0;
  const server = createServer(async (request, response) => {
    if (request.method === "PUT" && request.url === "/delay") {
      delayMs = Number(await text(request));
      response.statusCode = 204;
      response.end();
      return;
    }
    const { state, questions } = await json(request);
    await setTimeout(delayMs);
    const answers = Object.fromEntries(
      Object.entries(questions).map(([id, question]) => [
        id,
        { type: "noul", noul: labelNoul(state, question) },
      ]),
    );
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({ answers }));
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
  const fakeSystemOne = await startFakeSystemOne();
  const fakeSystemOneUrl = `http://127.0.0.1:${fakeSystemOne.address().port}/v1/systemone`;

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
    fakeSystemOne.close();
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

    const botPort = await findFreePort();
    const bot = spawnProcess(
      "bot",
      "uv",
      [
        "run",
        "python",
        "-m",
        "moq_chat_moderation.bot",
        "--port",
        String(botPort),
        "--relay-url",
        moqtUrl,
        "--insecure",
        "--system-one-url",
        fakeSystemOneUrl,
      ],
      { cwd: botDir },
    );
    childProcesses.push(bot);
    await waitForOutput(bot, /serving rooms on/, "bot", 180_000);

    await runCommand(
      resolveCommandName("npm"),
      ["run", "e2e:chat-moderation"],
      {
        cwd: jsDir,
        env: {
          ...process.env,
          MEDIA_E2E_BASE_URL: baseUrl,
          CHAT_MODERATION_E2E_MOQT_URL: moqtUrl,
          CHAT_MODERATION_E2E_BOT_URL: `http://127.0.0.1:${botPort}`,
          CHAT_MODERATION_E2E_SYSTEM_ONE_ORIGIN: `http://127.0.0.1:${fakeSystemOne.address().port}`,
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
