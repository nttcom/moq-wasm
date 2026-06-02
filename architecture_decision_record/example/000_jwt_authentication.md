# ADR 000: Example - JWT Authentication

## Status
Accepted

## Date
2026-06-02

## Context
We need to decide on an authentication method for a web application that requires user authentication.
The following options are under consideration:

- Session-based authentication
- JWT (JSON Web Token) based authentication
- OAuth 2.0 + OpenID Connect

The system is designed for a multi-server architecture with horizontal scaling,
and mobile app API access is planned for the future.

## Decision
Adopt **JWT (JSON Web Token) based authentication**.

Storing tokens on the client side keeps the server stateless, which is well-suited for horizontal scaling.
It also provides a standardized interface for integration with mobile apps, web clients, and external services.

## Consequences

### Positive
- No server-side session store needed, making scale-out straightforward
- Simple propagation of authentication information between microservices
- Provides a unified authentication flow for mobile apps, web, and external clients

### Negative
- Immediate token revocation is difficult (requires additional implementation such as blacklist management)
- Token size grows as more information is included in the payload, adding per-request overhead
- Signing key management and rotation must be properly maintained

### Neutral
- Token lifetimes: access token 15 minutes, refresh token 7 days
- Refresh tokens are stored in the database with Refresh Token Rotation (RTR)

## Alternatives Considered

### Session-based authentication
Simple and proven, but requires a shared session store (e.g. Redis) in a multi-server setup,
increasing infrastructure complexity. Deemed unsuitable for future scale-out requirements.

### OAuth 2.0 + OpenID Connect
Easy integration with external IdPs (e.g. Google), but the cost of building and operating a self-hosted IdP is high.
現時点ではソーシャルログイン要件がないため、オーバースペックと判断した。
将来要件が発生した際に再検討する。

## References
- [RFC 7519 - JSON Web Token (JWT)](https://datatracker.ietf.org/doc/html/rfc7519)
- [OWASP JWT Security Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/JSON_Web_Token_for_Java_Cheat_Sheet.html)
- [Refresh Token Rotation](https://auth0.com/docs/secure/tokens/refresh-tokens/refresh-token-rotation)