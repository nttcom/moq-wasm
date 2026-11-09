#!/usr/bin/env node

import { runSystemOneBotE2E } from "./system-one-bot-e2e.mjs";

/// Answers every choice question with the choice `PUT /answer` set, and every boxes question
/// (`Every <target>.`) with the boxes `PUT /boxes` set for its target: `{"<target>": [[left, top,
/// right, bottom], ...]}`.
function answerQuestions({ questions }, { answer, boxes }) {
  return Object.fromEntries(
    Object.entries(questions).map(([id, question]) => {
      if (question.type !== "boxes") {
        return [id, { choice: answer }];
      }
      const target = question.instructions.replace(/^Every (.*)\.$/, "$1");
      const found = (boxes[target] ?? []).map(([left, top, right, bottom]) => ({
        box: { left, top, right, bottom },
        found: 0.9,
      }));
      return [id, { boxes: found, count: found.length }];
    }),
  );
}

runSystemOneBotE2E({
  name: "camera-detection",
  answer: answerQuestions,
  settings: { answer: "はい", boxes: {} },
});
