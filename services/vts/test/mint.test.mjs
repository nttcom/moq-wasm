import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { promisify } from "node:util";

import { parseApps } from "../src/apps.mjs";
import { verifyToken } from "../src/verify.mjs";

const run = promisify(execFile);
const MINT = new URL("../bin/mint.mjs", import.meta.url).pathname;

async function writeAppsFile(entries) {
  const dir = await mkdtemp(join(tmpdir(), "vts-mint-"));
  const path = join(dir, "apps.json");
  await writeFile(path, JSON.stringify(entries));
  return path;
}

test("minted token verifies with the claims given on the command line", async () => {
  // Arrange
  const entries = [{ appId: "APP", secret: "s", isRelay: false }];
  const appsPath = await writeAppsFile(entries);

  // Act
  const { stdout } = await run(process.execPath, [
    MINT,
    "--apps",
    appsPath,
    "--app-id",
    "APP",
    "--publish",
    "site1/cam1",
    "--subscribe",
    "",
    "--ttl",
    "30m",
  ]);
  const result = await verifyToken(
    stdout.trim(),
    parseApps(JSON.stringify(entries)),
  );

  // Assert
  assert.equal(result.verified.claims.publish, "site1/cam1");
  assert.equal(result.verified.claims.subscribe, "");
  assert.ok(
    result.verified.claims.exp - Math.floor(Date.now() / 1000) <= 30 * 60,
  );
});

test("unknown appId exits with status 1", async () => {
  // Arrange
  const appsPath = await writeAppsFile([
    { appId: "APP", secret: "s", isRelay: false },
  ]);

  // Act
  const failure = await run(process.execPath, [
    MINT,
    "--apps",
    appsPath,
    "--app-id",
    "NOBODY",
  ]).catch((error) => error);

  // Assert
  assert.equal(failure.code, 1);
});
