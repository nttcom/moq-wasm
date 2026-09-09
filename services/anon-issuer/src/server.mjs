import http from "node:http";

import { RateLimiter } from "./rate_limiter.mjs";
import { signAnonToken } from "./sign.mjs";

export function createAnonIssuerServer(config, options = {}) {
  const rateLimiter = new RateLimiter(config.rateLimitPerMinute);
  const now = options.now ?? (() => Math.floor(Date.now() / 1000));
  return http.createServer((request, response) => {
    handle(request, response, config, rateLimiter, now).catch((error) => {
      console.error("unhandled error", error);
      sendJson(response, 500, { error: "internal_error" });
    });
  });
}

async function handle(request, response, config, rateLimiter, now) {
  const origin = request.headers.origin;
  const originAllowed =
    typeof origin === "string" && config.allowedOrigins.includes(origin);
  if (originAllowed) {
    response.setHeader("access-control-allow-origin", origin);
    response.setHeader("vary", "origin");
  }

  if (request.method === "GET" && request.url === "/healthz") {
    sendJson(response, 200, { status: "ok" });
    return;
  }
  if (request.url !== "/anon-token") {
    sendJson(response, 404, { error: "not_found" });
    return;
  }
  if (request.method === "OPTIONS") {
    response.setHeader("access-control-allow-methods", "POST");
    response.setHeader("access-control-allow-headers", "content-type");
    response.setHeader("access-control-max-age", "600");
    response.writeHead(204);
    response.end();
    return;
  }
  if (request.method !== "POST") {
    sendJson(response, 405, { error: "method_not_allowed" });
    return;
  }
  if (typeof origin === "string" && !originAllowed) {
    sendJson(response, 403, { error: "origin_not_allowed" });
    return;
  }
  const clientIp = clientAddress(request, config.trustProxy);
  if (!rateLimiter.allow(clientIp)) {
    console.info(`rate limited: ${clientIp}`);
    sendJson(response, 429, { error: "rate_limited" });
    return;
  }
  const issued = await signAnonToken(config, now());
  console.info(`issued anon token: ip=${clientIp} exp=${issued.expiresAt}`);
  sendJson(response, 200, issued);
}

function clientAddress(request, trustProxy) {
  if (trustProxy) {
    const forwarded = request.headers["x-forwarded-for"];
    if (typeof forwarded === "string" && forwarded !== "") {
      return forwarded.split(",")[0].trim();
    }
  }
  return request.socket.remoteAddress ?? "unknown";
}

function sendJson(response, status, body) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(body));
}
