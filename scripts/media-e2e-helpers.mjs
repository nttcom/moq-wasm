import { X509Certificate, createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

export const repoRoot = resolve(__dirname, "..");
export const jsDir = resolve(repoRoot, "js");
export const mediaPublisherPath =
  "/moq-wasm/examples/media/publisher/index.html";
export const mediaSubscriberPath =
  "/moq-wasm/examples/media/subscriber/index.html";
export const mediaIndexPath = "/moq-wasm/examples/media/index.html";
export const serverKeysDir = resolve(repoRoot, "moqt-server-sample", "keys");
export const certPath = resolve(serverKeysDir, "cert.pem");
export const keyPath = resolve(serverKeysDir, "key.pem");

export function ensureLinuxEnvironment() {
  if (process.platform !== "linux") {
    throw new Error("The automated media E2E flow is supported on Linux only.");
  }
}

export function resolveCommandName(command) {
  return process.platform === "win32" ? `${command}.cmd` : command;
}

export function assertPathExists(path, label, helpText) {
  if (!existsSync(path)) {
    const suffix = helpText ? ` ${helpText}` : "";
    throw new Error(`${label} not found: ${path}.${suffix}`.trim());
  }
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

export function computeCertificateSpkiBase64(targetCertPath = certPath) {
  assertPathExists(
    targetCertPath,
    "TLS certificate",
    "Run node scripts/setup-media-e2e.mjs first.",
  );
  const certificatePem = readFileSync(targetCertPath, "utf8");
  const certificate = new X509Certificate(certificatePem);
  const spkiDer = certificate.publicKey.export({ type: "spki", format: "der" });
  return createHash("sha256").update(spkiDer).digest("base64");
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
