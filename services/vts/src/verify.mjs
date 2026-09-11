import { compactVerify, decodeJwt, errors } from "jose";

import { secretKey } from "./key.mjs";

export async function verifyToken(token, apps) {
  let claims;
  try {
    claims = decodeJwt(token);
  } catch {
    return { error: "malformed_token" };
  }
  const appId = claims.appId;
  if (typeof appId !== "string" || appId === "") {
    return { error: "malformed_token" };
  }
  const app = apps.get(appId);
  if (!app) {
    return { error: "unknown_app" };
  }
  try {
    await compactVerify(token, secretKey(app.secret), {
      algorithms: ["HS256"],
    });
  } catch (error) {
    return { error: classifyVerifyError(error) };
  }
  return { verified: { appId, isRelay: app.isRelay, claims } };
}

function classifyVerifyError(error) {
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
