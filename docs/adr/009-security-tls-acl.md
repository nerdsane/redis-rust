# ADR-009: Security Features - TLS and ACL

## Status

Accepted

## Context

The redis-rust server currently has no authentication or encryption. The README warns:

> **Security Warning**: This server has **no authentication or access control**. Do NOT expose to untrusted networks or the public internet.

For production use cases, we need:
1. **TLS encryption** - Protect data in transit
2. **Authentication** - Verify client identity
3. **Authorization** - Control what clients can do

Redis 6.0+ introduced a comprehensive ACL (Access Control List) system that provides:
- Multiple users with different permissions
- Per-user command restrictions (categories, individual commands)
- Per-user key pattern restrictions
- Password-based authentication (SHA256 hashed)

## Decision

Implement optional security features via Cargo feature flags:
- `tls` - TLS encryption using rustls (tokio-rustls)
- `acl` - Redis 6.0+ compatible ACL system
- `security` - Bundle enabling both TLS and ACL

### TLS Implementation

Use rustls via tokio-rustls for TLS:
- **Why rustls**: Pure Rust, no OpenSSL dependency, good performance, memory-safe
- **MaybeSecureStream**: Enum wrapper allowing both plain and TLS connections
- **Configuration**: Environment variables for certificate paths

### ACL Implementation

Redis 6.0+ compatible ACL with:
- **AclUser**: User model with passwords, command/key permissions
- **AclManager**: User management and permission checking
- **Command Categories**: @read, @write, @admin, @dangerous, etc.
- **Key Patterns**: Glob-style patterns (~user:*, ~cache:*)
- **Commands**: AUTH, ACL WHOAMI, ACL LIST, ACL SETUSER, ACL DELUSER, ACL CAT, ACL GENPASS

### Feature Flags

```toml
[features]
tls = ["tokio-rustls", "rustls-pemfile"]
acl = ["sha2"]
security = ["tls", "acl"]
```

When features are disabled:
- ACL commands return sensible defaults (e.g., ACL WHOAMI returns "default")
- No runtime overhead from disabled features
- Code compiles without security dependencies

## Consequences

### Positive

- Production deployments can enable security as needed
- No overhead for development/testing (features optional)
- Compatible with Redis clients expecting ACL commands
- TLS uses modern, memory-safe implementation (rustls)
- Modular design allows TLS-only or ACL-only configurations

### Negative

- Full ACL integration requires connection handler refactoring
- TLS adds latency for handshake (~1-2ms per connection)
- ACL permission checking adds small overhead per command
- More configuration complexity for operators

### Risks

- ACL implementation may have gaps vs Redis behavior
- TLS certificate management is operator responsibility
- Connection handler TLS integration is not complete (TODO)

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-01-09 | Initial ADR created | Security features required for production use |
| 2026-01-09 | Use rustls over native-tls | Pure Rust, no system dependencies, memory-safe |
| 2026-01-09 | Redis 6.0+ ACL compatibility | Industry standard, familiar to operators |
| 2026-01-09 | Optional via feature flags | Zero overhead when not needed, flexible deployment |

## Implementation Status

### Implemented

| Component | Location | Status |
|-----------|----------|--------|
| ACL data model | `src/security/acl/user.rs` | Complete |
| AclManager | `src/security/acl/mod.rs` | Complete |
| Key pattern matching | `src/security/acl/patterns.rs` | Complete |
| ACL command handlers | `src/security/acl/commands.rs` | Complete |
| TLS config | `src/security/tls/config.rs` | Complete |
| MaybeSecureStream | `src/security/tls/stream.rs` | Complete |
| AUTH command | `src/redis/commands.rs` | Parsing complete |
| ACL commands | `src/redis/commands.rs` | Parsing complete |
| Server configuration | `src/production/server_config.rs` | Complete |
| CLI/env documentation | `src/bin/server_optimized.rs` | Complete |

### Validated

- ACL data model tests (19 tests passing)
- Password hashing (SHA256)
- Command category permissions
- Key pattern glob matching
- ACL rule parsing (SETUSER rules)

### Not Yet Implemented

| Component | Notes |
|-----------|-------|
| Connection handler TLS integration | Requires making handler generic over stream type |
| ACL permission checking in handler | Need to add check before command execution |
| ACL file loading | Load ACL configuration from file at startup |
| TLS accept loop integration | Need to wrap TcpStream with TlsAcceptor |
| Client certificate authentication | Extract identity from client cert |

## References

- [Redis ACL Documentation](https://redis.io/docs/management/security/acl/)
- [rustls](https://github.com/rustls/rustls)
- [tokio-rustls](https://github.com/rustls/tokio-rustls)
