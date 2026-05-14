#!/usr/bin/env node

import { spawn } from "node:child_process";
import {
  assertPathExists,
  certPath,
  ensureLinuxEnvironment,
  getDefaultBaseUrl,
  getDefaultMoqtUrl,
  getDefaultWebPort,
  getErrorMessage,
  jsDir,
  keyPath,
  mediaIndexPath,
  repoRoot,
  resolveCommandName,
  waitForHttpOk,
} from "./media-e2e-helpers.mjs";

const childProcesses = [];

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
    "Run node scripts/setup-media-e2e.mjs first.",
  );
  assertPathExists(
    `${jsDir}/pkg/moqt_client_wasm.js`,
    "bindings/wasm build output",
    "Run node scripts/setup-media-e2e.mjs first.",
  );

  const webPort = getDefaultWebPort();
  const baseUrl = getDefaultBaseUrl();
  const namespace = process.env.MEDIA_E2E_NAMESPACE ?? `e2e/${Date.now()}`;
  const moqtUrl = getDefaultMoqtUrl();

  const cleanup = async () => {
    await Promise.allSettled(
      [...childProcesses].reverse().map((child) => terminateProcess(child)),
    );
  };

  registerSignalHandlers(cleanup);

  try {
    const server = spawnProcess(
      "server",
      "cargo",
      ["run", "-p", "relay"],
      {
        cwd: repoRoot,
      },
    );
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

    childProcesses.push(server, vite);

    await Promise.all([
      waitForOutput(server, /Relay server started/, "relay", 180_000),
      waitForHttpOk(`${baseUrl}${mediaIndexPath}`, 120_000),
    ]);

    await runCommand(resolveCommandName("npm"), ["run", "e2e:media"], {
      cwd: jsDir,
      env: {
        ...process.env,
        MEDIA_E2E_BASE_URL: baseUrl,
        MEDIA_E2E_MOQT_URL: moqtUrl,
        MEDIA_E2E_NAMESPACE: namespace,
      },
    });
  } finally {
    await cleanup();
  }
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

function waitForOutput(child, pattern, label, timeoutMs) {
  return new Promise((resolvePromise, rejectPromise) => {
    let buffer = "";
    const streams = [child.stdout, child.stderr].filter(Boolean);

    const timer = setTimeout(() => {
      cleanup();
      rejectPromise(
        new Error(`Timed out waiting for ${label} to become ready.`),
      );
    }, timeoutMs);

    const onData = (chunk) => {
      buffer += chunk.toString();
      if (pattern.test(buffer)) {
        cleanup();
        resolvePromise();
      }
    };

    const onExit = (code) => {
      cleanup();
      rejectPromise(
        new Error(
          `${label} exited before becoming ready (code ${code ?? "unknown"}).`,
        ),
      );
    };

    const cleanup = () => {
      clearTimeout(timer);
      child.off("exit", onExit);
      for (const stream of streams) {
        stream.off("data", onData);
      }
    };

    for (const stream of streams) {
      stream.on("data", onData);
    }
    child.on("exit", onExit);
  });
}

async function runCommand(command, args, options) {
  await new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env,
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
