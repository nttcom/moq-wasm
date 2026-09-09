export const TEST_ORIGIN = "https://example.test";

export function testConfig(overrides = {}) {
  return {
    appId: "anon",
    secret: "anon-secret",
    ttlSeconds: 3600,
    port: 0,
    allowedOrigins: [TEST_ORIGIN],
    rateLimitPerMinute: 3,
    trustProxy: false,
    ...overrides,
  };
}

export async function startServer(server) {
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return `http://127.0.0.1:${server.address().port}`;
}

export function stopServer(server) {
  return new Promise((resolve) => server.close(resolve));
}
