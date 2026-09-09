import assert from "node:assert/strict";
import { test } from "node:test";

import { jwtVerify } from "jose";

import { createAnonIssuerServer } from "../src/server.mjs";
import {
  TEST_ORIGIN,
  startServer,
  stopServer,
  testConfig,
} from "./support.mjs";

const NOW = 1_759_600_000;

async function withServer(config, run) {
  const server = createAnonIssuerServer(config, { now: () => NOW });
  const baseUrl = await startServer(server);
  try {
    await run(baseUrl);
  } finally {
    await stopServer(server);
  }
}

function postToken(baseUrl, headers = {}) {
  return fetch(`${baseUrl}/anon-token`, { method: "POST", headers });
}

test("issues a token that verifies with the shared secret", async () => {
  await withServer(testConfig(), async (baseUrl) => {
    // Act
    const response = await postToken(baseUrl);
    const body = await response.json();
    const { payload } = await jwtVerify(
      body.token,
      new TextEncoder().encode("anon-secret"),
      { algorithms: ["HS256"], currentDate: new Date(NOW * 1000) },
    );

    // Assert
    assert.equal(response.status, 200);
    assert.equal(body.expiresAt, NOW + 3600);
    assert.equal(payload.appId, "anon");
    assert.equal(payload.publish, "");
    assert.equal(payload.subscribe, "");
    assert.equal(payload.iat, NOW);
    assert.equal(payload.exp, NOW + 3600);
  });
});

test("rate limits a client that exceeds the per-minute budget", async () => {
  await withServer(testConfig({ rateLimitPerMinute: 2 }), async (baseUrl) => {
    // Arrange
    await postToken(baseUrl);
    await postToken(baseUrl);

    // Act
    const response = await postToken(baseUrl);

    // Assert
    assert.equal(response.status, 429);
  });
});

test("allowed origin receives CORS headers", async () => {
  await withServer(testConfig(), async (baseUrl) => {
    // Act
    const response = await postToken(baseUrl, { origin: TEST_ORIGIN });

    // Assert
    assert.equal(response.status, 200);
    assert.equal(
      response.headers.get("access-control-allow-origin"),
      TEST_ORIGIN,
    );
  });
});

test("disallowed origin is refused without CORS headers", async () => {
  await withServer(testConfig(), async (baseUrl) => {
    // Act
    const response = await postToken(baseUrl, { origin: "https://evil.test" });

    // Assert
    assert.equal(response.status, 403);
    assert.equal(response.headers.get("access-control-allow-origin"), null);
  });
});

test("preflight for an allowed origin succeeds", async () => {
  await withServer(testConfig(), async (baseUrl) => {
    // Act
    const response = await fetch(`${baseUrl}/anon-token`, {
      method: "OPTIONS",
      headers: {
        origin: TEST_ORIGIN,
        "access-control-request-method": "POST",
      },
    });

    // Assert
    assert.equal(response.status, 204);
    assert.equal(response.headers.get("access-control-allow-methods"), "POST");
  });
});

test("forwarded address is used as the rate limit key when the proxy is trusted", async () => {
  await withServer(
    testConfig({ rateLimitPerMinute: 1, trustProxy: true }),
    async (baseUrl) => {
      // Arrange
      await postToken(baseUrl, { "x-forwarded-for": "10.0.0.1" });

      // Act
      const otherClient = await postToken(baseUrl, {
        "x-forwarded-for": "10.0.0.2",
      });
      const sameClient = await postToken(baseUrl, {
        "x-forwarded-for": "10.0.0.1",
      });

      // Assert
      assert.equal(otherClient.status, 200);
      assert.equal(sameClient.status, 429);
    },
  );
});

test("GET /healthz returns 200", async () => {
  await withServer(testConfig(), async (baseUrl) => {
    // Act
    const response = await fetch(`${baseUrl}/healthz`);

    // Assert
    assert.equal(response.status, 200);
  });
});
