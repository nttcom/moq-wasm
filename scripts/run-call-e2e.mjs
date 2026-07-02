#!/usr/bin/env node
// Orchestration runner for call E2E tests.
// Starts redis + relay-a (4433) + relay-b (4434) via docker compose,
// launches vite preview, then runs the Playwright call-e2e spec.
//
// If relay-a and relay-b containers serving ports 4433/4434 are already running
// (e.g. from a prior test run or a sibling compose project), the runner re-uses
// them and skips docker compose up to avoid port-allocation conflicts.

import { spawn, execFileSync } from "node:child_process";
import {
  assertPathExists,
  certPath,
  ensureLinuxEnvironment,
  keyPath,
  getDefaultBaseUrl,
  getDefaultWebPort,
  getErrorMessage,
  jsDir,
  repoRoot,
  resolveCommandName,
  waitForHttpOk,
} from "./media-e2e-helpers.mjs";
import { resolveLocalRelayUrl } from "./resolve-local-relay-url.mjs";

const callIndexPath = "/moq-wasm/examples/call/index.html";
const defaultRelayAUrl = "https://127.0.0.1:4433";
const defaultRelayBUrl = "https://127.0.0.1:4434";

const childProcesses = [];
const playwrightArgs = process.argv.slice(2);
// Track whether we started docker services ourselves (so we can stop them).
let ownedDockerServices = false;

async function main() {
  ensureLinuxEnvironment();
  assertPathExists(
    certPath,
    "TLS certificate",
    "Run node scripts/setup-media-e2e.mjs first.",
  );
  assertPathExists(
    keyPath,
    "TLS private key",
    "Run node scripts/setup-media-e2e.mjs first.",
  );
  assertPathExists(
    `${jsDir}/node_modules`,
    "examples/browser/node_modules",
    "Run npm install in examples/browser first.",
  );
  assertPathExists(
    `${jsDir}/pkg/moqt_client_wasm.js`,
    "bindings/wasm build output",
    "Run node scripts/setup-media-e2e.mjs first.",
  );

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
    if (ownedDockerServices) {
      await runCommand(
        resolveCommandName("docker"),
        ["compose", "stop", "relay-a", "relay-b", "redis"],
        { cwd: repoRoot },
      ).catch(() => {});
    }
  };

  registerSignalHandlers(cleanup);

  try {
    const relaysAlreadyRunning = areRelayPortsAlreadyBound();

    if (relaysAlreadyRunning) {
      console.error(
        "[setup] Relay ports 4433/4434 already occupied — reusing existing relay containers.",
      );
    } else {
      // Reuse a prebuilt relay image when one is already present (e.g. pulled from
      // the registry in CI, or built by a previous local run); otherwise build it.
      if (relayImageExists()) {
        console.error(
          "[setup] Reusing existing moqt-relay:local image (skipping build).",
        );
      } else {
        console.error("[setup] Building relay docker image...");
        await runCommand(
          resolveCommandName("docker"),
          ["compose", "build", "relay-common"],
          { cwd: repoRoot },
        );
      }

      console.error(
        "[setup] Starting redis, relay-a, relay-b via docker compose...",
      );
      await runCommand(
        resolveCommandName("docker"),
        ["compose", "up", "-d", "redis", "relay-a", "relay-b"],
        { cwd: repoRoot },
      );
      ownedDockerServices = true;
    }

    // When reusing existing containers the relays are already bound to their
    // ports, so they are ready by definition. For newly-started containers
    // we follow the compose logs until both emit "Relay server started".
    const relayReadyPromise = relaysAlreadyRunning
      ? Promise.resolve()
      : waitForRelayReadyFollow(repoRoot, 180_000);

    // Start vite preview server.
    const vite = spawnProcess(
      "vite",
      resolveCommandName("npm"),
      [
        "exec",
        "vite",
        "--",
        "--host",
        "127.0.0.1",
        "--port",
        String(webPort),
        "--strictPort",
      ],
      { cwd: jsDir },
    );
    childProcesses.push(vite);

    await Promise.all([
      relayReadyPromise,
      waitForHttpOk(`${baseUrl}${callIndexPath}`, 120_000),
    ]);

    const relayAUrl = getCallRelayUrl("CALL_E2E_RELAY_A_URL", defaultRelayAUrl);
    const relayBUrl = getCallRelayUrl("CALL_E2E_RELAY_B_URL", defaultRelayBUrl);
    console.error(`[setup] Using call relay URLs: ${relayAUrl}, ${relayBUrl}`);

    await runCommand(
      resolveCommandName("npm"),
      playwrightArgs.length > 0
        ? ["run", "e2e:call", "--", ...playwrightArgs]
        : ["run", "e2e:call"],
      {
        cwd: jsDir,
        env: {
          ...process.env,
          MEDIA_E2E_BASE_URL: baseUrl,
          CALL_E2E_RELAY_A_URL: relayAUrl,
          CALL_E2E_RELAY_B_URL: relayBUrl,
        },
      },
    );
  } finally {
    await cleanup();
  }
}

function getCallRelayUrl(envName, defaultUrl) {
  const configuredUrl = process.env[envName];
  const url = configuredUrl ?? resolveLocalRelayUrl(defaultUrl).toString();
  return stripTrailingSlash(url);
}

function stripTrailingSlash(url) {
  return url.endsWith("/") ? url.slice(0, -1) : url;
}

// Return true if the relay image used by docker compose is already available
// locally (pulled from the registry in CI, or built by an earlier run).
function relayImageExists() {
  try {
    execFileSync("docker", ["image", "inspect", "moqt-relay:local"], {
      stdio: "ignore",
    });
    return true;
  } catch (_error) {
    return false;
  }
}

// Return true if docker containers already have UDP 4433 and 4434 bound.
// Uses docker ps port output; not a UDP-level probe.
function areRelayPortsAlreadyBound() {
  try {
    const out = execFileSync("docker", ["ps", "--format", "{{.Ports}}"], {
      encoding: "utf8",
    });
    // Docker reports "0.0.0.0:4433->443/udp" style entries.
    const has4433 = out.includes(":4433->443/udp");
    const has4434 = out.includes(":4434->443/udp");
    return has4433 && has4434;
  } catch (_error) {
    return false;
  }
}

// For newly-started containers: follow logs until two "Relay server started"
// lines appear, then stop tailing.
function waitForRelayReadyFollow(cwd, timeoutMs) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(
      resolveCommandName("docker"),
      ["compose", "logs", "--follow", "--no-color", "relay-a", "relay-b"],
      {
        cwd,
        env: process.env,
        detached: process.platform !== "win32",
        stdio: ["ignore", "pipe", "pipe"],
      },
    );

    pipeOutput(child.stdout, process.stderr, "relay-logs");
    pipeOutput(child.stderr, process.stderr, "relay-logs");

    let buffer = "";
    let found = 0;

    const timer = setTimeout(() => {
      cleanupChild();
      rejectPromise(
        new Error(
          `Timed out waiting for ${2} relay(s) to become ready (found ${found}).`,
        ),
      );
    }, timeoutMs);

    const onData = (chunk) => {
      buffer += chunk.toString();
      const matches = buffer.match(/Relay server started/g);
      found = matches ? matches.length : 0;
      if (found >= 2) {
        cleanupChild();
        resolvePromise();
      }
    };

    const onExit = (code) => {
      clearTimeout(timer);
      rejectPromise(
        new Error(
          `docker compose logs process exited before relays became ready (code ${code ?? "unknown"}).`,
        ),
      );
    };

    const cleanupChild = () => {
      clearTimeout(timer);
      child.off("exit", onExit);
      // Stop tailing without killing the relay containers themselves.
      try {
        if (process.platform !== "win32" && typeof child.pid === "number") {
          process.kill(-child.pid, "SIGTERM");
        } else {
          child.kill("SIGTERM");
        }
      } catch (_error) {
        // ignore — child may have already exited
      }
    };

    child.stdout?.on("data", onData);
    child.stderr?.on("data", onData);
    child.on("exit", onExit);
  });
}

function registerSignalHandlers(cleanup) {
  const handler = async () => {
    await cleanup();
    process.exit(130);
  };

  process.once("SIGINT", handler);
  process.once("SIGTERM", handler);
}

function spawnProcess(label, command, args, options) {
  const child = spawn(command, args, {
    cwd: options.cwd,
    env: process.env,
    detached: process.platform !== "win32",
    stdio: ["ignore", "pipe", "pipe"],
  });

  pipeOutput(child.stdout, process.stdout, label);
  pipeOutput(child.stderr, process.stderr, label);

  child.on("error", (error) => {
    process.stderr.write(
      `[${label}] failed to start: ${getErrorMessage(error)}\n`,
    );
  });

  return child;
}

function pipeOutput(stream, destination, label) {
  if (!stream) {
    return;
  }

  stream.on("data", (chunk) => {
    const text = chunk.toString();
    const prefixed = text
      .split("\n")
      .filter(
        (line, index, lines) => line.length > 0 || index < lines.length - 1,
      )
      .map((line) => `[${label}] ${line}`)
      .join("\n");
    if (prefixed.length > 0) {
      destination.write(`${prefixed}\n`);
    }
  });
}

async function runCommand(command, args, options) {
  await new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env ?? process.env,
      stdio: "inherit",
    });

    child.on("error", (error) => {
      rejectPromise(
        new Error(`${command} failed to start: ${getErrorMessage(error)}`),
      );
    });
    child.on("exit", (code) => {
      if (code === 0) {
        resolvePromise();
        return;
      }
      rejectPromise(
        new Error(`${command} ${args.join(" ")} exited with code ${code}.`),
      );
    });
  });
}

async function terminateProcess(child) {
  if (!child || child.exitCode !== null) {
    return;
  }

  await new Promise((resolvePromise) => {
    let settled = false;
    const finish = () => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(forceKillTimer);
      clearTimeout(resolveTimer);
      child.off("exit", onExit);
      resolvePromise();
    };
    const onExit = () => {
      finish();
    };
    const forceKillTimer = setTimeout(() => {
      if (child.exitCode === null) {
        killProcessTree(child, "SIGKILL");
      }
    }, 5_000);
    const resolveTimer = setTimeout(() => {
      finish();
    }, 7_000);

    child.on("exit", onExit);
    killProcessTree(child, "SIGTERM");
    if (child.exitCode !== null) {
      finish();
    }
  });
}

function killProcessTree(child, signal) {
  if (process.platform !== "win32" && typeof child.pid === "number") {
    try {
      process.kill(-child.pid, signal);
      return;
    } catch (_error) {
      // Fall back to killing the direct child below.
    }
  }
  child.kill(signal);
}

main().catch((error) => {
  console.error(getErrorMessage(error));
  process.exitCode = 1;
});
