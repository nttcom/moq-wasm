import { signToken } from "../src/sign.mjs";

export const APP_ID = "APP";
export const APP_SECRET = "app-secret";
export const RELAY_ID = "RELAY";
export const RELAY_SECRET = "relay-secret";
export const NOW = 1_759_600_000;

export function testApps() {
  return new Map([
    [APP_ID, { secret: APP_SECRET, isRelay: false }],
    [RELAY_ID, { secret: RELAY_SECRET, isRelay: true }],
  ]);
}

export function appToken(claims = {}, options = {}) {
  return signToken(
    { appId: APP_ID, publish: "site1", subscribe: "site1", ...claims },
    options.secret ?? APP_SECRET,
    { now: options.now ?? NOW, ttlSeconds: options.ttlSeconds ?? 3600 },
  );
}
