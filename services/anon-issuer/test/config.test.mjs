import assert from "node:assert/strict";
import { test } from "node:test";

import { configFromEnv } from "../src/config.mjs";

const VALID_ENV = {
  ANON_SECRET: "s",
  ANON_ALLOWED_ORIGINS: "https://a.test, https://b.test",
};

test("reads the configuration with defaults", () => {
  // Act
  const config = configFromEnv(VALID_ENV);

  // Assert
  assert.equal(config.appId, "anon");
  assert.equal(config.ttlSeconds, 43200);
  assert.equal(config.port, 8080);
  assert.deepEqual(config.allowedOrigins, ["https://a.test", "https://b.test"]);
  assert.equal(config.rateLimitPerMinute, 30);
  assert.equal(config.trustProxy, false);
});

test("missing secret is an error", () => {
  // Act / Assert
  assert.throws(
    () => configFromEnv({ ANON_ALLOWED_ORIGINS: "https://a.test" }),
    /ANON_SECRET/,
  );
});

test("ttl above 24 hours is an error", () => {
  // Act / Assert
  assert.throws(
    () => configFromEnv({ ...VALID_ENV, ANON_TOKEN_TTL_SECONDS: "86401" }),
    /ANON_TOKEN_TTL_SECONDS/,
  );
});

test("missing allowed origins is an error", () => {
  // Act / Assert
  assert.throws(
    () => configFromEnv({ ANON_SECRET: "s" }),
    /ANON_ALLOWED_ORIGINS/,
  );
});

test("wildcard origin is an error", () => {
  // Act / Assert
  assert.throws(
    () => configFromEnv({ ANON_SECRET: "s", ANON_ALLOWED_ORIGINS: "*" }),
    /explicit origins/,
  );
});
