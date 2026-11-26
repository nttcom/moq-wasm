#!/usr/bin/env node

import { runPageE2E } from "./relay-dev.mjs";

runPageE2E("message", {
  MESSAGE_E2E_NAMESPACE:
    process.env.MESSAGE_E2E_NAMESPACE ?? `anon/e2e/${Date.now()}`,
});
