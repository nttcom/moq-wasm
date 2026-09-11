import assert from "node:assert/strict";
import { test } from "node:test";

import { signToken } from "../src/sign.mjs";
import { verifyToken } from "../src/verify.mjs";
import {
  APP_ID,
  NOW,
  RELAY_ID,
  RELAY_SECRET,
  appToken,
  testApps,
} from "./support.mjs";

const apps = testApps();

test("valid token yields the appId, the relay flag and the raw claims", async () => {
  // Arrange
  const token = await appToken({ publish: "site1/cam1", subscribe: "site1" });

  // Act
  const result = await verifyToken(token, apps);

  // Assert
  assert.deepEqual(result, {
    verified: {
      appId: APP_ID,
      isRelay: false,
      claims: {
        appId: APP_ID,
        publish: "site1/cam1",
        subscribe: "site1",
        iat: NOW,
        exp: NOW + 3600,
      },
    },
  });
});

test("relay app is flagged as relay", async () => {
  // Arrange
  const token = await signToken(
    { appId: RELAY_ID, publish: "", subscribe: "" },
    RELAY_SECRET,
    {
      now: NOW,
      ttlSeconds: 60,
    },
  );

  // Act
  const result = await verifyToken(token, apps);

  // Assert
  assert.equal(result.verified.isRelay, true);
});

test("time claims are passed through, not judged", async () => {
  // Arrange
  const expired = await appToken({}, { now: NOW - 7200, ttlSeconds: 60 });

  // Act
  const result = await verifyToken(expired, apps);

  // Assert
  assert.equal(result.verified.claims.exp, NOW - 7200 + 60);
});

test("token that is not a JWT is malformed", async () => {
  // Act / Assert
  assert.deepEqual(await verifyToken("not-a-jwt", apps), {
    error: "malformed_token",
  });
});

test("token without appId is malformed", async () => {
  // Arrange
  const token = await signToken({ publish: "" }, "app-secret", {
    now: NOW,
    ttlSeconds: 60,
  });

  // Act / Assert
  assert.deepEqual(await verifyToken(token, apps), {
    error: "malformed_token",
  });
});

test("unknown appId is rejected before the signature is checked", async () => {
  // Arrange
  const token = await appToken({ appId: "NOBODY" });

  // Act / Assert
  assert.deepEqual(await verifyToken(token, apps), { error: "unknown_app" });
});

test("token signed with another secret is rejected", async () => {
  // Arrange
  const token = await appToken({}, { secret: "wrong-secret" });

  // Act / Assert
  assert.deepEqual(await verifyToken(token, apps), {
    error: "invalid_signature",
  });
});

test("unsigned token is rejected", async () => {
  // Arrange
  const { UnsecuredJWT } = await import("jose");
  const token = new UnsecuredJWT({ appId: APP_ID })
    .setIssuedAt(NOW)
    .setExpirationTime(NOW + 60)
    .encode();

  // Act
  const result = await verifyToken(token, apps);

  // Assert
  assert.ok(result.error);
});
