# Plan: CI Updates + Stateright Nightly + ACL Test Fixes

## Changes

### 1. Update `.github/workflows/ci.yml` — Add WAL tests + WAL DST

Add two new steps after existing "Unit tests (with ACL feature)":
- **WAL DST tests**: `cargo test --release --test wal_dst_test` (multi-seed crash+fault injection)
- **WAL unit tests**: Already covered by `cargo test --lib` since WAL tests are in the lib

Update the build step to also build with WAL support. Update test counts in comments.

Remove `continue-on-error: true` from the ACL Tcl test step — it either passes or the CI should fail. If it's expected to partially fail, we should skip the known-broken tests explicitly rather than silently swallowing failures.

### 2. Create `.github/workflows/stateright-nightly.yml` — Exhaustive model checking

New nightly workflow that runs all 4 Stateright model checks with LARGER state bounds:
- `stateright_replication_model_check`
- `stateright_persistence_model_check`
- `stateright_anti_entropy_model_check`
- `stateright_wal_durability_model_check`

Schedule: nightly at 3 AM UTC. Also triggerable manually.
Uses larger config constants for deeper exploration (e.g., max_writes=6, max_segments=5).

### 3. Update DST soak script to include WAL

Add Phase 4 to `scripts/soak-dst.sh`: WAL DST soak with random seeds.

### 4. Fix ACL Tcl test — remove `continue-on-error` and handle properly

The ACL Tcl test (`unit/acl`) runs in internal mode. It partially passes (~25 tests) but crashes at ACL LOG/DRYRUN (unimplemented commands). Two approaches:

**Option A (recommended)**: Add the `--tags` filter to skip tests requiring unimplemented features. The Tcl suite supports tag-based filtering. We already skip `-needs:debug -needs:repl -needs:save`. Add `-needs:acl-log` or use `--dont_clean` to handle the crash gracefully.

**Option B**: Stub ACL LOG and ACL DRYRUN as error-returning commands so they don't crash the harness.

I'll go with Option A since it's less invasive — skip tests we can't pass yet, but ensure the ones we CAN pass don't regress.

### 5. Add WAL integration test to CI (optional, docker-based)

Add a separate job `wal-integration` that builds docker images and runs `scripts/run-wal-integration.sh`. This is heavier so make it optional/manual-trigger.

## Files to modify:
- `.github/workflows/ci.yml` — add WAL tests, fix ACL handling
- `.github/workflows/stateright-nightly.yml` — NEW: nightly model checking
- `scripts/soak-dst.sh` — add WAL phase
- `scripts/run-redis-compat.sh` — potentially adjust ACL tag filtering
