import assert from "node:assert/strict";
import { after, before, test } from "node:test";

import { createVtsServer } from "../src/server.mjs";
import { NOW, appToken, testApps } from "./support.mjs";

let server;
let baseUrl;

before(async () => {
  server = createVtsServer(testApps());
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  baseUrl = `http://127.0.0.1:${server.address().port}`;
});

after(() => new Promise((resolve) => server.close(resolve)));

async function postVerify(body) {
  return fetch(`${baseUrl}/verify`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body,
  });
}

test("POST /verify returns 200 with the claims for a valid token", async () => {
  // Arrange
  const token = await appToken();

  // Act
  const response = await postVerify(JSON.stringify({ token }));

  // Assert
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), {
    appId: "APP",
    isRelay: false,
    claims: {
      appId: "APP",
      publish: "site1",
      subscribe: "site1",
      iat: NOW,
      exp: NOW + 3600,
    },
  });
});

test("POST /verify returns 401 with the reason for a rejected token", async () => {
  // Arrange
  const token = await appToken({}, { secret: "wrong" });

  // Act
  const response = await postVerify(JSON.stringify({ token }));

  // Assert
  assert.equal(response.status, 401);
  assert.deepEqual(await response.json(), { error: "invalid_signature" });
});

test("POST /verify returns 400 when the body has no token", async () => {
  // Act
  const response = await postVerify(JSON.stringify({}));

  // Assert
  assert.equal(response.status, 400);
});

test("POST /verify returns 400 for a non-JSON body", async () => {
  // Act
  const response = await postVerify("token=abc");

  // Assert
  assert.equal(response.status, 400);
});

test("GET /healthz returns 200", async () => {
  // Act
  const response = await fetch(`${baseUrl}/healthz`);

  // Assert
  assert.equal(response.status, 200);
});

test("unknown routes return 404", async () => {
  // Act
  const response = await fetch(`${baseUrl}/verify`);

  // Assert
  assert.equal(response.status, 404);
});
