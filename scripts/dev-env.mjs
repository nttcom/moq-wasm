import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

export const repoRoot = resolve(__dirname, "..");
export const serverKeysDir = resolve(repoRoot, "crates", "relay", "keys");
export const certPath = resolve(serverKeysDir, "cert.pem");
export const keyPath = resolve(serverKeysDir, "key.pem");

export function ensureLinuxEnvironment() {
  if (process.platform !== "linux" && process.platform !== "darwin") {
    throw new Error(
      "The automated media E2E flow is supported on Linux and macOS only.",
    );
  }
}

export function resolveCommandName(command) {
  return process.platform === "win32" ? `${command}.cmd` : command;
}

export function getErrorMessage(error) {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  try {
    return JSON.stringify(error);
  } catch (_error) {
    return String(error);
  }
}
