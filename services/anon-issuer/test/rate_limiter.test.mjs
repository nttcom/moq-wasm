import assert from "node:assert/strict";
import { test } from "node:test";

import { RateLimiter } from "../src/rate_limiter.mjs";

test("allows up to the limit within a minute and refuses the next", () => {
  // Arrange
  const limiter = new RateLimiter(2);
  const t0 = 1_000_000;

  // Act / Assert
  assert.equal(limiter.allow("ip", t0), true);
  assert.equal(limiter.allow("ip", t0 + 1000), true);
  assert.equal(limiter.allow("ip", t0 + 2000), false);
});

test("a new minute resets the budget", () => {
  // Arrange
  const limiter = new RateLimiter(1);
  const t0 = 1_000_000;
  limiter.allow("ip", t0);

  // Act / Assert
  assert.equal(limiter.allow("ip", t0 + 60_000), true);
});

test("keys are counted independently", () => {
  // Arrange
  const limiter = new RateLimiter(1);

  // Act / Assert
  assert.equal(limiter.allow("a", 0), true);
  assert.equal(limiter.allow("b", 0), true);
  assert.equal(limiter.allow("a", 0), false);
});
