// The VTS, the relay and vite, run natively by the browser e2e runners whose
// pages dial one relay.

import {
  registerSignalHandlers,
  runCommand,
  spawnProcess,
  spawnViteServer,
  terminateAll,
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
  getDefaultBaseUrl,
  getDefaultMoqtUrl,
  getDefaultWebPort,
  jsDir,
  waitForHttpOk,
} from "./helpers.mjs";
import { nativeRelayEnv, startVts } from "./vts-dev.mjs";

/// `ready` resolves once the relay is up and vite serves `pagePath`.
/// `cleanup` terminates `childProcesses`, which the caller may push more
/// processes to, in reverse order, then runs `onCleanup`; SIGINT and SIGTERM
/// run it too.
export function startRelayAndPage(pagePath, { timeoutMs, onCleanup } = {}) {
  const childProcesses = [];
  const cleanup = async () => {
    await terminateAll(childProcesses);
    onCleanup?.();
  };
  registerSignalHandlers(cleanup);

  const ready = (async () => {
    childProcesses.push(await startVts());
    const server = spawnProcess("server", "cargo", ["run", "-p", "relay"], {
      cwd: repoRoot,
      env: { ...process.env, ...nativeRelayEnv() },
    });
    const vite = spawnViteServer(getDefaultWebPort());
    childProcesses.push(server, vite);
    await Promise.all([
      waitForOutput(server, /Relay server started/, "relay", timeoutMs ?? 180_000),
      waitForHttpOk(`${getDefaultBaseUrl()}${pagePath}`, timeoutMs ?? 120_000),
    ]);
  })();
  // A caller may await `ready` only after other startup work; until then a
  // rejection must not end the process as unhandled, skipping `cleanup`.
  ready.catch(() => {});

  return { childProcesses, cleanup, ready };
}

/// The spec reads the relay URL from `<PAGE>_E2E_MOQT_URL`; `env` entries
/// override the defaults. Sets `process.exitCode = 1` on failure.
export function runPageE2E(page, env = {}) {
  const main = async () => {
    ensureLinuxEnvironment();
    assertE2EPrerequisites();
    const { cleanup, ready } = startRelayAndPage(
      `/moq-wasm/examples/${page}/index.html`,
    );
    try {
      await ready;
      await runCommand(resolveCommandName("npm"), ["run", `e2e:${page}`], {
        cwd: jsDir,
        env: {
          ...process.env,
          MEDIA_E2E_BASE_URL: getDefaultBaseUrl(),
          [`${page.toUpperCase()}_E2E_MOQT_URL`]: getDefaultMoqtUrl(),
          ...env,
        },
      });
    } finally {
      await cleanup();
    }
  };
  main().catch((error) => {
    console.error(getErrorMessage(error));
    process.exitCode = 1;
  });
}
