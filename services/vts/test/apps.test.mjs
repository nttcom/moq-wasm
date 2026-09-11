import assert from "node:assert/strict";
import { test } from "node:test";

import { parseApps } from "../src/apps.mjs";

test("parses entries into a map keyed by appId", () => {
  // Arrange
  const text = JSON.stringify([
    { appId: "a", secret: "s1", isRelay: false },
    { appId: "r", secret: "s2", isRelay: true },
  ]);

  // Act
  const apps = parseApps(text);

  // Assert
  assert.deepEqual(apps.get("a"), { secret: "s1", isRelay: false });
  assert.deepEqual(apps.get("r"), { secret: "s2", isRelay: true });
});

test("rejects a duplicated appId", () => {
  // Arrange
  const text = JSON.stringify([
    { appId: "a", secret: "s1", isRelay: false },
    { appId: "a", secret: "s2", isRelay: false },
  ]);

  // Act / Assert
  assert.throws(() => parseApps(text), /duplicated/);
});

test("rejects a missing secret", () => {
  // Act / Assert
  assert.throws(
    () => parseApps(JSON.stringify([{ appId: "a", isRelay: false }])),
    /secret/,
  );
});

test("rejects a non-boolean isRelay", () => {
  // Act / Assert
  assert.throws(
    () =>
      parseApps(JSON.stringify([{ appId: "a", secret: "s", isRelay: "yes" }])),
    /isRelay/,
  );
});

test("rejects a document that is not an array", () => {
  // Act / Assert
  assert.throws(() => parseApps("{}"), /array/);
});
