export const MAX_TOKEN_TTL_SECONDS = 24 * 60 * 60;

export function configFromEnv(env) {
  const secret = env.ANON_SECRET;
  if (typeof secret !== "string" || secret === "") {
    throw new Error("ANON_SECRET is required");
  }
  const ttlSeconds = Number(env.ANON_TOKEN_TTL_SECONDS ?? 43200);
  if (!Number.isInteger(ttlSeconds) || ttlSeconds <= 0) {
    throw new Error("ANON_TOKEN_TTL_SECONDS must be a positive integer");
  }
  if (ttlSeconds > MAX_TOKEN_TTL_SECONDS) {
    throw new Error(
      `ANON_TOKEN_TTL_SECONDS must not exceed ${MAX_TOKEN_TTL_SECONDS}`,
    );
  }
  const allowedOrigins = (env.ANON_ALLOWED_ORIGINS ?? "")
    .split(",")
    .map((origin) => origin.trim())
    .filter((origin) => origin !== "");
  if (allowedOrigins.length === 0) {
    throw new Error("ANON_ALLOWED_ORIGINS is required");
  }
  if (allowedOrigins.includes("*")) {
    throw new Error("ANON_ALLOWED_ORIGINS must list explicit origins, not *");
  }
  const rateLimitPerMinute = Number(env.ANON_RATE_LIMIT_PER_MINUTE ?? 30);
  if (!Number.isInteger(rateLimitPerMinute) || rateLimitPerMinute <= 0) {
    throw new Error("ANON_RATE_LIMIT_PER_MINUTE must be a positive integer");
  }
  return {
    appId: env.ANON_APP_ID ?? "anon",
    secret,
    ttlSeconds,
    port: Number(env.ANON_ISSUER_PORT ?? 8080),
    allowedOrigins,
    rateLimitPerMinute,
    trustProxy: env.ANON_TRUST_PROXY === "true",
  };
}
