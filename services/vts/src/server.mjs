import http from "node:http";

import { verifyToken } from "./verify.mjs";

const MAX_BODY_BYTES = 16 * 1024;

export function createVtsServer(apps) {
  return http.createServer((request, response) => {
    handle(request, response, apps).catch((error) => {
      console.error("unhandled error", error);
      sendJson(response, 500, { error: "internal_error" });
    });
  });
}

async function handle(request, response, apps) {
  if (request.method === "GET" && request.url === "/healthz") {
    sendJson(response, 200, { status: "ok" });
    return;
  }
  if (request.method !== "POST" || request.url !== "/verify") {
    sendJson(response, 404, { error: "not_found" });
    return;
  }
  const body = await readJsonBody(request);
  if (body === undefined || typeof body.token !== "string") {
    sendJson(response, 400, { error: "invalid_request" });
    return;
  }
  const result = await verifyToken(body.token, apps);
  if (result.error) {
    console.info(`verify rejected: ${result.error}`);
    sendJson(response, 401, { error: result.error });
    return;
  }
  console.info(
    `verify ok: appId=${result.verified.appId} isRelay=${result.verified.isRelay}`,
  );
  sendJson(response, 200, result.verified);
}

async function readJsonBody(request) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > MAX_BODY_BYTES) {
      return undefined;
    }
    chunks.push(chunk);
  }
  try {
    const parsed = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    return parsed !== null && typeof parsed === "object" ? parsed : undefined;
  } catch {
    return undefined;
  }
}

function sendJson(response, status, body) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(body));
}
