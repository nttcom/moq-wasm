import { spawn } from "node:child_process";

import { getErrorMessage } from "./media-e2e-helpers.mjs";

export function registerSignalHandlers(cleanup) {
  const handler = async () => {
    await cleanup();
    process.exit(130);
  };

  process.once("SIGINT", handler);
  process.once("SIGTERM", handler);
}

export function spawnProcess(label, command, args, options) {
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

export function waitForOutput(child, pattern, label, timeoutMs) {
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

export async function runCommand(command, args, options) {
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

export async function terminateProcess(child) {
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
