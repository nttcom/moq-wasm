#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { repoRoot, resolveCommandName } from "./media-e2e-helpers.mjs";

const defaultMoqtUrl = "https://127.0.0.1:4433";
const dockerRelayHostEnvNames = ["MOQT_DOCKER_RELAY_HOST", "LOCAL_RELAY_HOST"];

function main() {
  const moqtUrl = process.argv[2] || process.env.MOQT_URL || defaultMoqtUrl;
  const resolvedUrl = resolveLocalRelayUrl(moqtUrl);
  console.log(formatUrl(resolvedUrl));
}

export function resolveLocalRelayUrl(moqtUrl) {
  const url = new URL(moqtUrl || defaultMoqtUrl);
  const overrideHost = findOverrideHost();
  if (overrideHost) {
    url.hostname = overrideHost;
    return url;
  }

  if (!isLoopbackHost(url.hostname) || !isDockerComposeRelayRunning()) {
    return url;
  }

  const dockerDesktopHost = detectDockerDesktopBridgeHost();
  if (dockerDesktopHost) {
    url.hostname = dockerDesktopHost;
  }
  return url;
}

function findOverrideHost() {
  for (const name of dockerRelayHostEnvNames) {
    const value = process.env[name]?.trim();
    if (value) {
      return value;
    }
  }
  return null;
}

function isLoopbackHost(hostname) {
  const normalized = hostname.toLowerCase();
  return (
    normalized === "localhost" ||
    normalized === "127.0.0.1" ||
    normalized === "::1" ||
    normalized === "[::1]"
  );
}

function isDockerComposeRelayRunning() {
  try {
    const output = execFileSync(
      resolveCommandName("docker"),
      ["compose", "ps", "--status", "running", "--services", "relay-a"],
      {
        cwd: repoRoot,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      },
    );
    return output.split(/\s+/).includes("relay-a");
  } catch (_error) {
    return false;
  }
}

function detectDockerDesktopBridgeHost() {
  if (process.platform !== "darwin") {
    return null;
  }

  try {
    const routeTable = execFileSync("netstat", ["-rn", "-f", "inet"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
    return selectBridgeHost(routeTable);
  } catch (_error) {
    return null;
  }
}

function selectBridgeHost(routeTable) {
  const candidates = routeTable
    .split("\n")
    .map((line) => line.trim().split(/\s+/))
    .filter((columns) => columns.length >= 4)
    .filter((columns) => /^bridge\d+$/.test(columns[3]))
    .map((columns) => columns[0])
    .filter(isPrivateIpv4Host);

  return (
    candidates.find((host) => host.endsWith(".2")) ?? candidates[0] ?? null
  );
}

function isPrivateIpv4Host(host) {
  const octets = host.split(".").map((part) => Number(part));
  if (
    octets.length !== 4 ||
    octets.some((octet) => !Number.isInteger(octet) || octet < 0 || octet > 255)
  ) {
    return false;
  }

  const [first, second] = octets;
  return (
    first === 10 ||
    (first === 172 && second >= 16 && second <= 31) ||
    (first === 192 && second === 168)
  );
}

function formatUrl(url) {
  const formatted = url.toString();
  if (url.pathname === "/" && !url.search && !url.hash) {
    return formatted.slice(0, -1);
  }
  return formatted;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main();
}
