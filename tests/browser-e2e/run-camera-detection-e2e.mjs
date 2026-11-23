#!/usr/bin/env node

import { answerQuestions, runSystemOneBotE2E } from "./system-one-bot-e2e.mjs";

runSystemOneBotE2E({
  name: "camera-detection",
  answer: answerQuestions,
  settings: { answer: "はい", boxes: {} },
  // One bot at a time, as deployed, so a page in a second room learns that no bot is free.
  maxRunning: 1,
});
