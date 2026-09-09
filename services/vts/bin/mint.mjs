#!/usr/bin/env node
import { parseArgs } from "node:util";

import { loadApps } from "../src/apps.mjs";
import { signToken } from "../src/sign.mjs";

const USAGE = `usage: mint.mjs --apps <apps.json> --app-id <id> [--publish <path>] [--subscribe <path>] [--ttl <duration>]

  --publish / --subscribe  relative namespace path; "" grants everything under the appId
  --ttl                    e.g. 30m, 12h, 365d (default 12h)`;

const TTL_UNITS = { s: 1, m: 60, h: 3600, d: 86400 };

function parseTtl(text) {
  const match = /^(\d+)([smhd])$/.exec(text);
  if (!match) {
    throw new Error(`invalid --ttl "${text}"`);
  }
  return Number(match[1]) * TTL_UNITS[match[2]];
}

const { values } = parseArgs({
  options: {
    apps: { type: "string" },
    "app-id": { type: "string" },
    publish: { type: "string" },
    subscribe: { type: "string" },
    ttl: { type: "string", default: "12h" },
    help: { type: "boolean", default: false },
  },
});

if (values.help || !values.apps || !values["app-id"]) {
  console.error(USAGE);
  process.exit(values.help ? 0 : 2);
}

const apps = await loadApps(values.apps);
const app = apps.get(values["app-id"]);
if (!app) {
  console.error(`unknown appId "${values["app-id"]}" in ${values.apps}`);
  process.exit(1);
}

const claims = { appId: values["app-id"] };
if (values.publish !== undefined) {
  claims.publish = values.publish;
}
if (values.subscribe !== undefined) {
  claims.subscribe = values.subscribe;
}

const token = await signToken(claims, app.secret, {
  now: Math.floor(Date.now() / 1000),
  ttlSeconds: parseTtl(values.ttl),
});
console.log(token);
