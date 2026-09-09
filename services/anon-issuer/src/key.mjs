export function secretKey(secret) {
  return new TextEncoder().encode(secret);
}
