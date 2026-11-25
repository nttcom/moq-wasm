# ADR 000: Example - JWT Authentication

> [!WARNING]
> This document is an example. No relation to product.

## Status
Accepted

## Date
2026-06-02

## Context
A horizontally scaled, multi-server web application needs user authentication and must support future mobile app API access.

Options considered:
- Session-based authentication
- JWT (JSON Web Token) authentication
- OAuth 2.0 + OpenID Connect

## Decision
Adopt **JWT (JSON Web Token) authentication**.

Storing tokens on the client keeps servers stateless for horizontal scaling and provides a uniform interface across web, mobile, and external clients.

## Consequences

### Positive
- No server-side session store needed for scale-out
- Straightforward auth propagation between microservices
- Single authentication flow across web, mobile, and external clients

### Negative
- Immediate revocation requires extra state (e.g., a denylist)
- Larger payloads increase per-request header overhead
- Requires signing-key management and rotation

### Neutral
- Lifetimes: access token 15 minutes, refresh token 7 days
- Refresh tokens are persisted in the database with Refresh Token Rotation (RTR)

## Alternatives Considered

- **Session-based authentication**: Simple and proven, but requires a shared session store (e.g., Redis) across servers, adding infrastructure complexity for scale-out.
- **OAuth 2.0 + OpenID Connect**: Integrates cleanly with external IdPs (e.g., Google), but operating a self-hosted IdP is overkill without a current social-login requirement. Revisit if social login is needed later.

## References
- [RFC 7519 - JSON Web Token (JWT)](https://datatracker.ietf.org/doc/html/rfc7519)
- [OWASP JWT Security Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/JSON_Web_Token_for_Java_Cheat_Sheet.html)
- [Refresh Token Rotation](https://auth0.com/docs/secure/tokens/refresh-tokens/refresh-token-rotation)