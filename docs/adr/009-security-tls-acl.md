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

- ACL implementation may have gaps vs Redis behavior (Tcl ACL tests blocked on ACL LOG, DRYRUN, channel permissions)
- TLS certificate management is operator responsibility
- Fast path bypass required careful handling — restricted users must not skip ACL key checks

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-01-09 | Initial ADR created | Security features required for production use |
| 2026-01-09 | Use rustls over native-tls | Pure Rust, no system dependencies, memory-safe |
| 2026-01-09 | Redis 6.0+ ACL compatibility | Industry standard, familiar to operators |
| 2026-01-09 | Optional via feature flags | Zero overhead when not needed, flexible deployment |
| 2026-01-09 | Make connection handler generic over stream type | Enables MaybeSecureStream for TLS/plain TCP |
| 2026-01-09 | TLS accept loop integration complete | Server wraps TcpStream with TlsAcceptor when configured |
| 2026-01-09 | ACL permission checking in handler | Per-command ACL check before execution |
| 2026-01-09 | Per-connection authentication state | Track authenticated user per connection |
| 2026-01-10 | ACL file loading implemented | Load users from ACL_FILE env at startup |
| 2026-01-10 | Client certificate authentication | CN from client cert maps to ACL user |
| 2026-02-16 | Fixed fast path ACL key bypass | Fast path skipped key permission checks for restricted users; gated on `user_has_unrestricted_keys()` |
| 2026-02-16 | Replaced dead executor ACL stubs | AUTH/WHOAMI/LIST/USERS/GETUSER/SETUSER/DELUSER in executor now `debug_assert!(false)` — these are handled at connection level |
| 2026-02-16 | Added `verify_invariants()` to AclManager | Checks default user exists/enabled, password hash validity, no empty usernames |
| 2026-02-16 | ACL DST harness with shadow state | `src/security/acl_dst.rs` — symbolic verification: shadow model as spec, 5 op types, 4 config presets |
| 2026-02-16 | Enabled `--features acl` in CI | 536 lib tests pass (507 base + 29 ACL), integration DST tests with 100+ seeds |

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
| Connection handler TLS | `src/production/connection_optimized.rs` | Complete (generic over stream) |
| TLS accept loop | `src/production/server_optimized.rs` | Complete |
| ACL permission checking | `src/production/connection_optimized.rs` | Complete |
| Per-connection auth state | `src/production/connection_optimized.rs` | Complete |
| AUTH/ACL command handling | `src/production/connection_optimized.rs` | Complete |
| Command.get_keys() | `src/redis/commands.rs` | Complete |
| AclManager in server | `src/production/server_optimized.rs` | Complete |
| ACL file loading | `src/security/acl/file.rs` | Complete |
| ACL file integration | `src/production/server_optimized.rs` | Complete |
| Client cert CN extraction | `src/security/tls/stream.rs` | Complete |
| Client cert authentication | `src/production/connection_optimized.rs` | Complete |
| Fast path ACL key gate | `src/production/connection_optimized.rs` | Complete |
| ACL `verify_invariants()` | `src/security/acl/mod.rs` | Complete |
| ACL DST harness | `src/security/acl_dst.rs` | Complete |
| ACL DST integration tests | `tests/acl_dst_test.rs` | Complete |
| CI `--features acl` step | `.github/workflows/ci.yml` | Complete |

### Validated

- ACL data model unit tests (29 ACL-specific tests passing with `--features acl`)
- Password hashing (SHA256)
- Command category permissions
- Key pattern glob matching
- ACL rule parsing (SETUSER rules)
- TLS feature compiles correctly (cargo check --features tls)
- Server accepts connections with/without TLS based on config
- ACL feature compiles correctly (cargo check --features acl)
- Security feature compiles correctly (cargo check --features security)
- AUTH command authenticates users and updates connection state
- ACL commands (WHOAMI, LIST, USERS, GETUSER, SETUSER, DELUSER, CAT, GENPASS) handled at connection level
- Permission checking rejects unauthorized commands
- ACL file parsing (7 tests passing)
- ACL file loading at startup loads users correctly
- Key pattern restrictions enforced for both fast path and regular path
- Client certificate authentication auto-authenticates users based on CN
- Client cert CN maps to ACL user name for authorization
- **ACL DST**: Shadow state verification across 400+ seeds (100 seeds x 4 configs), 500-5000 ops/seed
- **CI**: `cargo test --lib --features acl` runs 536 tests in GitHub Actions
- Fast path correctly bypassed for restricted key users (`has_unrestricted_keys()` gate)

### Known Gaps (Not In Scope)

- Tcl ACL tests (`acl.tcl`, `acl-v2.tcl`) — require ACL LOG, ACL DRYRUN, channel permissions
- Read/write key patterns (`%R~`, `%W~`) — parsed but not enforced
- ACL file hot-reload (`ACL LOAD`) — command not wired
- Transaction ACL re-check — permissions not re-verified during EXEC

## References

- [Redis ACL Documentation](https://redis.io/docs/management/security/acl/)
- [rustls](https://github.com/rustls/rustls)
- [tokio-rustls](https://github.com/rustls/tokio-rustls)
