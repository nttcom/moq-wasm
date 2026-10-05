import { existsSync } from "node:fs";
import { resolve } from "node:path";

import {
  certPath,
  getErrorMessage,
  keyPath,
  repoRoot,
} from "../../scripts/dev-env.mjs";

export const jsDir = resolve(repoRoot, "examples", "browser");
export const mediaIndexPath = "/moq-wasm/examples/media/index.html";
export const messageIndexPath = "/moq-wasm/examples/message/index.html";
export const liveViewerPath = "/moq-wasm/examples/live-viewer/index.html";
export const chatModerationPath =
  "/moq-wasm/examples/moq-chat-moderation/index.html";
export const cameraDetectionPath =
  "/moq-wasm/examples/moq-camera-detection/index.html";
const setupHelpText = "Run node tests/browser-e2e/setup-media-e2e.mjs first.";

function assertPathExists(path, label, helpText) {
  if (!existsSync(path)) {
    const suffix = helpText ? ` ${helpText}` : "";
    throw new Error(`${label} not found: ${path}.${suffix}`.trim());
  }
}

export function assertE2EPrerequisites() {
  assertPathExists(certPath, "TLS certificate", setupHelpText);
  assertPathExists(keyPath, "TLS private key", setupHelpText);
  assertPathExists(`${jsDir}/node_modules`, "node_modules", setupHelpText);
  assertPathExists(`${jsDir}/pkg/moqt.js`, "wasm build output", setupHelpText);
}

export function getDefaultWebPort() {
  const rawValue = process.env.MEDIA_E2E_WEB_PORT ?? "4173";
  const value = Number(rawValue);
  if (!Number.isInteger(value) || value <= 0) {
    throw new Error(`Invalid MEDIA_E2E_WEB_PORT value: ${rawValue}`);
  }
  return value;
}

export function getDefaultMoqtUrl() {
  return process.env.MEDIA_E2E_MOQT_URL ?? "https://127.0.0.1:4433";
}

export function getDefaultBaseUrl() {
  return (
    process.env.MEDIA_E2E_BASE_URL ?? `http://127.0.0.1:${getDefaultWebPort()}`
  );
}

export async function waitForHttpOk(url, timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) {
        return;
      }
      lastError = new Error(`HTTP ${response.status} ${response.statusText}`);
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 500));
  }
  throw new Error(
    `Timed out waiting for ${url}: ${getErrorMessage(lastError)}`,
  );
}
