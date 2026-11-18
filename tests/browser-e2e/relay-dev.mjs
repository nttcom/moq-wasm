// The VTS, the relay and vite, run natively by the browser e2e runners whose
// pages dial one relay.

import {
  registerSignalHandlers,
  spawnProcess,
  spawnViteServer,
  terminateProcess,
  waitForOutput,
} from "./process.mjs";
import { repoRoot } from "../../scripts/dev-env.mjs";
import {
  getDefaultBaseUrl,
  getDefaultWebPort,
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
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
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
