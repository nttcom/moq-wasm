import { SignJWT } from "jose";

import { secretKey } from "./key.mjs";

export async function signToken(claims, secret, { now, ttlSeconds }) {
  return new SignJWT(claims)
    .setProtectedHeader({ alg: "HS256" })
    .setIssuedAt(now)
    .setExpirationTime(now + ttlSeconds)
    .sign(secretKey(secret));
}
