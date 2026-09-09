import { decodeJwt, errors, jwtVerify } from "jose";

import { secretKey } from "./key.mjs";

export const DEFAULT_MAX_TOKEN_TTL_SECONDS = 24 * 60 * 60;
export const DEFAULT_CLOCK_LEEWAY_SECONDS = 60;

export async function verifyToken(token, apps, options = {}) {
  const now = options.now ?? Math.floor(Date.now() / 1000);
  const maxTokenTtlSeconds =
    options.maxTokenTtlSeconds ?? DEFAULT_MAX_TOKEN_TTL_SECONDS;
  const clockLeewaySeconds =
    options.clockLeewaySeconds ?? DEFAULT_CLOCK_LEEWAY_SECONDS;

  let unverified;
  try {
    unverified = decodeJwt(token);
  } catch {
    return { error: "malformed_token" };
  }
  const appId = unverified.appId;
  if (typeof appId !== "string" || appId === "") {
    return { error: "malformed_token" };
  }
  const app = apps.get(appId);
  if (!app) {
    return { error: "unknown_app" };
  }

  let payload;
  try {
    ({ payload } = await jwtVerify(token, secretKey(app.secret), {
      algorithms: ["HS256"],
      currentDate: new Date(now * 1000),
      clockTolerance: clockLeewaySeconds,
      requiredClaims: ["exp", "iat"],
    }));
  } catch (error) {
    return { error: classifyVerifyError(error) };
  }

  if (payload.iat > now + clockLeewaySeconds) {
    return { error: "not_yet_valid" };
  }
  if (!app.isRelay && payload.exp - payload.iat > maxTokenTtlSeconds) {
    return { error: "ttl_too_long" };
  }
  const publish = namespacePathClaim(payload.publish);
  const subscribe = namespacePathClaim(payload.subscribe);
  if (publish === undefined || subscribe === undefined) {
    return { error: "invalid_claims" };
  }

  return {
    claims: {
      appId,
      publish,
      subscribe,
      isRelay: app.isRelay,
      exp: payload.exp,
    },
  };
}

function classifyVerifyError(error) {
  if (error instanceof errors.JWTExpired) {
    return "expired";
  }
  if (error instanceof errors.JWTClaimValidationFailed) {
    return "invalid_claims";
  }
  if (
    error instanceof errors.JWSSignatureVerificationFailed ||
    error instanceof errors.JOSEAlgNotAllowed
  ) {
    return "invalid_signature";
  }
  if (error instanceof errors.JOSEError) {
    return "malformed_token";
  }
  throw error;
}

function namespacePathClaim(value) {
  if (value === undefined || value === null) {
    return null;
  }
  if (typeof value !== "string") {
    return undefined;
  }
  if (value !== "" && value.split("/").some((element) => element === "")) {
    return undefined;
  }
  return value;
}
