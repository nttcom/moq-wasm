#!/usr/bin/env node

import { runSystemOneBotE2E } from "./system-one-bot-e2e.mjs";

/// Labels a question about 「<name>」 as harmful when the chat state holds `e2e-<name>`.
function labelNoul(state, question) {
  const [, name] = question.instructions.match(/「(.+?)」/) ?? [];
  return name !== undefined && state.includes(`e2e-${name}`) ? 0.95 : 0.02;
}

runSystemOneBotE2E({
  name: "chat-moderation",
  answer: ({ state, questions }) =>
    Object.fromEntries(
      Object.entries(questions).map(([id, question]) => [
        id,
        { type: "noul", noul: labelNoul(state, question) },
      ]),
    ),
});
