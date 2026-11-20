import { once } from "node:events";
import { createServer } from "node:http";
import { resolve } from "node:path";
import { text } from "node:stream/consumers";
import { setTimeout } from "node:timers/promises";

import { runCommand } from "./process.mjs";
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
} from "./helpers.mjs";
import { localPythonBot, startBotDispatcher, temporaryAppSecret } from "./bot-dispatcher-dev.mjs";
import { startRelayAndPage } from "./relay-dev.mjs";

/// A fake System One answering each POST with `{ answers: answer(body, settings) }`. `PUT /<key>`
/// sets `settings[key]` to its body: as text if the setting is a string, else parsed as JSON.
/// `PUT /delay` delays every answer by that many milliseconds.
async function startFakeSystemOne(answer, settings) {
  const server = createServer(async (request, response) => {
    const body = await text(request);
    if (request.method === "PUT") {
      const key = request.url.slice(1);
      settings[key] =
        typeof settings[key] === "string" ? body : JSON.parse(body);
      response.statusCode = 204;
      response.end();
      return;
    }
    await setTimeout(settings.delay);
    response.setHeader("content-type", "application/json");
    response.end(
      JSON.stringify({ answers: answer(JSON.parse(body), settings) }),
    );
  });
  await once(server.listen(0, "127.0.0.1"), "listening");
  return server;
}

/// Starts the VTS, the relay, vite and a fake System One, then `bot-dispatcher`, which launches the bot in
/// `examples/python/moq-<name>` as the type `<name>` for the pages of a temporary app, then runs
/// `npm run e2e:<name>`, whose pages join that app. `startBeforeBot` starts whatever else the bot
/// needs once the relay is up and returns the extra environment for the test. `maxRunning` is the
/// type's `max_running`.
export function runSystemOneBotE2E(options) {
  run(options).catch((error) => {
    console.error(getErrorMessage(error));
    process.exitCode = 1;
  });
}

async function run({
  name,
  answer,
  settings = {},
  maxRunning = 4,
  startBeforeBot = async () => ({}),
}) {
  ensureLinuxEnvironment();
  assertE2EPrerequisites();

  const botDir = resolve(repoRoot, `examples/python/moq-${name}`);
  const envPrefix = name.toUpperCase().replaceAll("-", "_");
  const baseUrl = getDefaultBaseUrl();
  const moqtUrl = getDefaultMoqtUrl();
  const fakeSystemOne = await startFakeSystemOne(answer, {
    delay: 0,
    ...settings,
  });
  const fakeSystemOneOrigin = `http://127.0.0.1:${fakeSystemOne.address().port}`;

  process.env.VTS_TEMPORARY_APP_SECRET = temporaryAppSecret;
  const { childProcesses, cleanup, ready } = startRelayAndPage(
    `/moq-wasm/examples/moq-${name}/index.html`,
    { onCleanup: () => fakeSystemOne.close() },
  );

  try {
    await ready;

    // The first run builds the moq-wasm SDK into the bot's environment.
    await runCommand(resolveCommandName("uv"), ["sync"], { cwd: botDir });
    const temporaryAppsUrl = await startBotDispatcher(
      {
        [name]: {
          ...localPythonBot(botDir, `moq_${name.replaceAll("-", "_")}.bot`, [
            "--relay-url",
            moqtUrl,
            "--insecure",
            "--system-one-url",
            `${fakeSystemOneOrigin}/v1/systemone`,
          ]),
          max_running: maxRunning,
        },
      },
      childProcesses,
    );
    const issued = await fetch(`${temporaryAppsUrl}/tokens`, { method: "POST" });
    if (!issued.ok) {
      throw new Error(`temporary-apps answered ${issued.status}`);
    }
    const app = await issued.json();

    const extraEnv = await startBeforeBot({ botDir, moqtUrl, app, childProcesses });

    await runCommand(resolveCommandName("npm"), ["run", `e2e:${name}`], {
      cwd: jsDir,
      env: {
        ...process.env,
        MEDIA_E2E_BASE_URL: baseUrl,
        [`${envPrefix}_E2E_MOQT_URL`]: moqtUrl,
        [`${envPrefix}_E2E_TEMPORARY_APPS_URL`]: temporaryAppsUrl,
        [`${envPrefix}_E2E_TEMPORARY_APP_TOKEN`]: app.token,
        [`${envPrefix}_E2E_SYSTEM_ONE_ORIGIN`]: fakeSystemOneOrigin,
        ...extraEnv,
      },
    });
  } finally {
    await cleanup();
  }
}
