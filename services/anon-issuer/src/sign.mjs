import { SignJWT } from "jose";

import { secretKey } from "./key.mjs";

export async function signAnonToken({ appId, secret, ttlSeconds }, now) {
  const token = await new SignJWT({ appId, publish: "", subscribe: "" })
    .setProtectedHeader({ alg: "HS256" })
    .setIssuedAt(now)
    .setExpirationTime(now + ttlSeconds)
    .sign(secretKey(secret));
  return { token, expiresAt: now + ttlSeconds };
}
