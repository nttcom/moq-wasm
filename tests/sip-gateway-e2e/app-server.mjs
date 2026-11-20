#!/usr/bin/env node
// The app server the phone example (examples/browser/examples/phone) talks to,
// for a local SIP gateway only: it holds the app's secret, signs the page's
// token and the call tokens, places calls through the gateway and answers its
// call webhook with the room of the page that takes the call.
//
//   POST /token {room}                  -> {token, namespace}
//   POST /calls {to, room, peer}        -> the gateway's answer to POST /calls
//   GET  /incoming?room=                -> {call_id, from, to}, or 204 after 20 s
//   POST /answer {call_id, room, peer}  -> 204; the webhook answers with the room
//   POST /decline {call_id}             -> 204; the webhook answers decline
//   POST /webhook                       -> the gateway's call webhook
//
// Environment: APP_ID, APP_SECRET, GATEWAY_URL, PORT (8097).

import { createHmac, timingSafeEqual } from "node:crypto";
import { createServer } from "node:http";

const appId = required("APP_ID");
const secret = required("APP_SECRET");
const gatewayUrl = required("GATEWAY_URL");
const port = Number(process.env.PORT ?? 8097);
/// A call no page takes is declined as busy before the gateway's 60 s webhook timeout.
const RING_MS = 50_000;
const POLL_MS = 20_000;
const PAGE_TOKEN_SECONDS = 60 * 60;
const CALL_TOKEN_SECONDS = 60;

/// Calls the webhook received that no page has been offered yet, oldest first.
const waiting = [];
/// Pages waiting in GET /incoming.
const pollers = [];
/// The webhook responses still open, by call_id; `/answer`, `/decline` or `RING_MS` settles each.
const ringing = new Map();

function required(name) {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} must be set`);
  }
  return value;
}

function base64url(value) {
  return Buffer.from(value).toString("base64url");
}

function sign(claims) {
  const unsigned = `${base64url(JSON.stringify({ alg: "HS256", typ: "JWT" }))}.${base64url(JSON.stringify(claims))}`;
  return `${unsigned}.${createHmac("sha256", secret).update(unsigned).digest("base64url")}`;
}

function verified(token) {
  const [header, payload, signature] = (token ?? "").split(".");
  if (!signature) {
    return false;
  }
  const expected = createHmac("sha256", secret).update(`${header}.${payload}`).digest();
  const given = Buffer.from(signature, "base64url");
  return given.length === expected.length && timingSafeEqual(given, expected);
}

function now() {
  return Math.floor(Date.now() / 1000);
}

function send(response, status, body) {
  response.writeHead(status, {
    "access-control-allow-origin": "*",
    "content-type": "application/json",
  });
  response.end(body === undefined ? undefined : JSON.stringify(body));
}

async function readJson(request) {
  let text = "";
  for await (const chunk of request) {
    text += chunk;
  }
  return JSON.parse(text || "{}");
}

/// A relative room path: one or more non-empty elements.
function room(value) {
  if (typeof value !== "string" || value.split("/").some((element) => element === "")) {
    throw new Error("room must be a relative namespace path");
  }
  return value;
}

async function placeCall({ to, room: relative, peer }) {
  const token = sign({ appId, calls: room(relative), iat: now(), exp: now() + CALL_TOKEN_SECONDS });
  const response = await fetch(`${gatewayUrl}/calls`, {
    method: "POST",
    headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: JSON.stringify({ to, namespace_prefix: `${appId}/${relative}`, peer }),
  });
  return { status: response.status, body: await response.json() };
}

/// Hands the call to the first page waiting, or keeps it for the next one.
function offer(call) {
  const poller = pollers.shift();
  if (poller) {
    poller(call);
  } else {
    waiting.push(call);
  }
}

/// The next call for a page, or `undefined` after `POLL_MS` or once the page
/// is gone, so a closed page's request never takes a call.
function nextCall(response) {
  const call = waiting.shift();
  if (call) {
    return Promise.resolve(call);
  }
  return new Promise((resolve) => {
    const finish = (taken) => {
      clearTimeout(timer);
      const index = pollers.indexOf(finish);
      if (index >= 0) {
        pollers.splice(index, 1);
      }
      resolve(taken);
    };
    const timer = setTimeout(finish, POLL_MS);
    response.on("close", () => finish());
    pollers.push(finish);
  });
}

function decide(callId, decision) {
  const respond = ringing.get(callId);
  ringing.delete(callId);
  respond?.(decision);
}

async function handle(request, response) {
  const url = new URL(request.url, "http://localhost");
  if (request.method === "OPTIONS") {
    response.writeHead(204, {
      "access-control-allow-origin": "*",
      "access-control-allow-headers": "content-type",
      "access-control-allow-methods": "GET, POST",
    });
    return response.end();
  }
  if (request.method === "POST" && url.pathname === "/token") {
    const relative = room((await readJson(request)).room);
    const issued = now();
    const token = sign({ appId, publish: relative, subscribe: relative, iat: issued, exp: issued + PAGE_TOKEN_SECONDS });
    return send(response, 200, { token, namespace: [appId, ...relative.split("/")] });
  }
  if (request.method === "POST" && url.pathname === "/calls") {
    const placed = await placeCall(await readJson(request));
    return send(response, placed.status, placed.body);
  }
  if (request.method === "GET" && url.pathname === "/incoming") {
    const call = await nextCall(response);
    return call ? send(response, 200, call) : send(response, 204);
  }
  if (request.method === "POST" && url.pathname === "/answer") {
    const { call_id: callId, room: relative, peer } = await readJson(request);
    decide(callId, { namespace_prefix: `${appId}/${room(relative)}`, peer });
    return send(response, 204);
  }
  if (request.method === "POST" && url.pathname === "/decline") {
    decide((await readJson(request)).call_id, { reject: "decline" });
    return send(response, 204);
  }
  if (request.method === "POST" && url.pathname === "/webhook") {
    const token = request.headers.authorization?.replace(/^Bearer /, "");
    if (!verified(token)) {
      return send(response, 401, { error: "invalid_token" });
    }
    const call = await readJson(request);
    const decision = await new Promise((resolve) => {
      const timer = setTimeout(() => {
        ringing.delete(call.call_id);
        resolve({ reject: "busy" });
      }, RING_MS);
      ringing.set(call.call_id, (taken) => {
        clearTimeout(timer);
        resolve(taken);
      });
      offer({ call_id: call.call_id, from: call.from, to: call.to });
    });
    return send(response, 200, decision);
  }
  send(response, 404, { error: "not_found" });
}

createServer((request, response) => {
  handle(request, response).catch((error) => send(response, 400, { error: String(error.message ?? error) }));
}).listen(port, "127.0.0.1", () => console.log(`phone app server listening on ${port}`));
