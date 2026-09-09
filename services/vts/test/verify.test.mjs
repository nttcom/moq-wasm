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

test("valid token yields the claims", async () => {
  // Arrange
  const token = await appToken({ publish: "site1/cam1", subscribe: "site1" });

  // Act
  const result = await verifyToken(token, apps, { now: NOW + 10 });

  // Assert
  assert.deepEqual(result, {
    claims: {
      appId: APP_ID,
      publish: "site1/cam1",
      subscribe: "site1",
      isRelay: false,
      exp: NOW + 3600,
    },
  });
});

test("an absent claim is returned as null", async () => {
  // Arrange
  const token = await signToken(
    { appId: APP_ID, publish: "site1" },
    "app-secret",
    {
      now: NOW,
      ttlSeconds: 60,
    },
  );

  // Act
  const result = await verifyToken(token, apps, { now: NOW });

  // Assert
  assert.equal(result.claims.subscribe, null);
});

test("empty string claim is the app root and is kept as is", async () => {
  // Arrange
  const token = await appToken({ publish: "", subscribe: "" });

  // Act
  const result = await verifyToken(token, apps, { now: NOW });

  // Assert
  assert.equal(result.claims.publish, "");
  assert.equal(result.claims.subscribe, "");
});

test("token that is not a JWT is malformed", async () => {
  // Act / Assert
  assert.deepEqual(await verifyToken("not-a-jwt", apps, { now: NOW }), {
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
  assert.deepEqual(await verifyToken(token, apps, { now: NOW }), {
    error: "malformed_token",
  });
});

test("unknown appId is rejected before the signature is checked", async () => {
  // Arrange
  const token = await appToken({ appId: "NOBODY" });

  // Act / Assert
  assert.deepEqual(await verifyToken(token, apps, { now: NOW }), {
    error: "unknown_app",
  });
});

test("token signed with another secret is rejected", async () => {
  // Arrange
  const token = await appToken({}, { secret: "wrong-secret" });

  // Act / Assert
  assert.deepEqual(await verifyToken(token, apps, { now: NOW }), {
    error: "invalid_signature",
  });
});

test("expired token is rejected beyond the leeway", async () => {
  // Arrange
  const token = await appToken({}, { ttlSeconds: 60 });

  // Act / Assert
  assert.deepEqual(await verifyToken(token, apps, { now: NOW + 121 }), {
    error: "expired",
  });
});

test("token that expired within the leeway is still accepted", async () => {
  // Arrange
  const token = await appToken({}, { ttlSeconds: 60 });

  // Act
  const result = await verifyToken(token, apps, { now: NOW + 90 });

  // Assert
  assert.ok(result.claims);
});

test("token issued in the future is rejected beyond the leeway", async () => {
  // Arrange
  const token = await appToken({}, { now: NOW + 120 });

  // Act / Assert
  assert.deepEqual(await verifyToken(token, apps, { now: NOW }), {
    error: "not_yet_valid",
  });
});

test("client token longer than the maximum ttl is rejected", async () => {
  // Arrange
  const token = await appToken({}, { ttlSeconds: 24 * 3600 + 1 });

  // Act / Assert
  assert.deepEqual(await verifyToken(token, apps, { now: NOW }), {
    error: "ttl_too_long",
  });
});

test("relay token may outlive the maximum ttl", async () => {
  // Arrange
  const token = await signToken(
    { appId: RELAY_ID, publish: "", subscribe: "" },
    RELAY_SECRET,
    { now: NOW, ttlSeconds: 365 * 24 * 3600 },
  );

  // Act
  const result = await verifyToken(token, apps, { now: NOW });

  // Assert
  assert.equal(result.claims.isRelay, true);
  assert.equal(result.claims.exp, NOW + 365 * 24 * 3600);
});

test("path with an empty element is rejected", async () => {
  // Arrange
  const token = await appToken({ publish: "site1//cam1" });

  // Act / Assert
  assert.deepEqual(await verifyToken(token, apps, { now: NOW }), {
    error: "invalid_claims",
  });
});

test("non-string path claim is rejected", async () => {
  // Arrange
  const token = await appToken({ subscribe: ["site1"] });

  // Act / Assert
  assert.deepEqual(await verifyToken(token, apps, { now: NOW }), {
    error: "invalid_claims",
  });
});

test("token without exp is rejected", async () => {
  // Arrange
  const { SignJWT } = await import("jose");
  const token = await new SignJWT({ appId: APP_ID })
    .setProtectedHeader({ alg: "HS256" })
    .setIssuedAt(NOW)
    .sign(new TextEncoder().encode("app-secret"));

  // Act / Assert
  assert.deepEqual(await verifyToken(token, apps, { now: NOW }), {
    error: "invalid_claims",
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
  const result = await verifyToken(token, apps, { now: NOW });

  // Assert
  assert.ok(result.error);
});
