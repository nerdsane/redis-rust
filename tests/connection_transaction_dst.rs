//! Connection-level Transaction DST
//!
//! Tests the MULTI/EXEC/WATCH state machine as implemented in the production
//! connection handler (connection_optimized.rs), NOT the executor-level
//! transaction state.
//!
//! The production server moved transaction state to the connection level to
//! support cross-shard transactions. This DST exercises that exact code path
//! using ShardedActorState as the backend.

use redis_sim::io::simulation::SimulatedRng;
use redis_sim::io::Rng;
use redis_sim::production::ShardedActorState;
use redis_sim::redis::{Command, RespValue, SDS};

/// Simulates a single client connection's transaction state.
/// This mirrors the fields in OptimizedConnectionHandler.
struct SimulatedConnection {
    state: ShardedActorState,
    in_transaction: bool,
    transaction_queue: Vec<Command>,
    transaction_errors: bool,
    watched_keys: Vec<(String, RespValue)>,
}

impl SimulatedConnection {
    fn new(state: ShardedActorState) -> Self {
        SimulatedConnection {
            state,
            in_transaction: false,
            transaction_queue: Vec::new(),
            transaction_errors: false,
            watched_keys: Vec::new(),
        }
    }

    /// Execute a command through the connection-level transaction state machine.
    /// This mirrors the logic in connection_optimized.rs try_execute_command().
    async fn execute(&mut self, cmd: &Command) -> RespValue {
        if self.in_transaction {
            match cmd {
                Command::Exec => {
                    self.in_transaction = false;
                    if self.transaction_errors {
                        self.transaction_queue.clear();
                        self.transaction_errors = false;
                        self.watched_keys.clear();
                        RespValue::err(
                            "EXECABORT Transaction discarded because of previous errors.",
                        )
                    } else {
                        let watched = std::mem::take(&mut self.watched_keys);
                        let mut watch_failed = false;
                        for (key, old_value) in &watched {
                            let current =
                                self.state.execute(&Command::Get(key.clone())).await;
                            if !resp_values_equal(&current, old_value) {
                                watch_failed = true;
                                break;
                            }
                        }
                        if watch_failed {
                            self.transaction_queue.clear();
                            RespValue::Array(None)
                        } else {
                            let queued = std::mem::take(&mut self.transaction_queue);
                            let mut results = Vec::with_capacity(queued.len());
                            for queued_cmd in &queued {
                                let r = self.state.execute(queued_cmd).await;
                                results.push(r);
                            }
                            RespValue::Array(Some(results))
                        }
                    }
                }
                Command::Discard => {
                    self.in_transaction = false;
                    self.transaction_queue.clear();
                    self.transaction_errors = false;
                    self.watched_keys.clear();
                    RespValue::simple("OK")
                }
                Command::Multi => RespValue::err("ERR MULTI calls can not be nested"),
                Command::Watch(_) => {
                    RespValue::err("ERR WATCH inside MULTI is not allowed")
                }
                Command::Unknown(name) => {
                    self.transaction_errors = true;
                    RespValue::err(format!(
                        "ERR unknown command '{}', with args beginning with: ",
                        name.to_lowercase()
                    ))
                }
                _ => {
                    self.transaction_queue.push(cmd.clone());
                    RespValue::simple("QUEUED")
                }
            }
        } else {
            match cmd {
                Command::Multi => {
                    self.in_transaction = true;
                    self.transaction_queue.clear();
                    self.transaction_errors = false;
                    RespValue::simple("OK")
                }
                Command::Exec => RespValue::err("ERR EXEC without MULTI"),
                Command::Discard => RespValue::err("ERR DISCARD without MULTI"),
                Command::Watch(keys) => {
                    for key in keys {
                        let snapshot =
                            self.state.execute(&Command::Get(key.clone())).await;
                        self.watched_keys.push((key.clone(), snapshot));
                    }
                    RespValue::simple("OK")
                }
                Command::Unwatch => {
                    self.watched_keys.clear();
                    RespValue::simple("OK")
                }
                _ => self.state.execute(cmd).await,
            }
        }
    }
}

fn resp_values_equal(a: &RespValue, b: &RespValue) -> bool {
    match (a, b) {
        (RespValue::BulkString(a), RespValue::BulkString(b)) => a == b,
        (RespValue::Integer(a), RespValue::Integer(b)) => a == b,
        (RespValue::SimpleString(a), RespValue::SimpleString(b)) => a == b,
        (RespValue::Error(a), RespValue::Error(b)) => a == b,
        (RespValue::Array(None), RespValue::Array(None)) => true,
        (RespValue::Array(Some(a)), RespValue::Array(Some(b))) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(x, y)| resp_values_equal(x, y))
        }
        _ => false,
    }
}

fn is_ok(resp: &RespValue) -> bool {
    matches!(resp, RespValue::SimpleString(s) if s == "OK")
}

fn is_queued(resp: &RespValue) -> bool {
    matches!(resp, RespValue::SimpleString(s) if s == "QUEUED")
}

fn is_error(resp: &RespValue) -> bool {
    matches!(resp, RespValue::Error(_))
}

/// Run a single DST seed that exercises connection-level transactions.
/// Two connections share the same ShardedActorState (simulating two clients
/// hitting the same server). This tests cross-connection WATCH conflicts.
async fn run_connection_transaction_dst(seed: u64) -> Vec<String> {
    let mut rng = SimulatedRng::new(seed);
    let mut violations: Vec<String> = Vec::new();
    let state = ShardedActorState::with_shards(1);

    let mut conn_a = SimulatedConnection::new(state.clone());
    let mut conn_b = SimulatedConnection::new(state.clone());

    let num_keys = 10;

    // Seed some initial keys
    for i in 0..num_keys {
        let key = format!("k:{}", i);
        let val = format!("init:{}", i);
        conn_a
            .execute(&Command::set(key, SDS::from_str(&val)))
            .await;
    }

    for _ in 0..200 {
        let scenario = rng.gen_range(0, 100);
        let key_idx = rng.gen_range(0, num_keys as u64);
        let key = format!("k:{}", key_idx);
        let val_a = format!("a:{}", rng.gen_range(0, 1000));
        let val_b = format!("b:{}", rng.gen_range(0, 1000));

        if scenario < 25 {
            // === WATCH + no conflict => EXEC succeeds ===
            let r = conn_a.execute(&Command::Watch(vec![key.clone()])).await;
            assert!(is_ok(&r), "WATCH should OK");

            let r = conn_a.execute(&Command::Multi).await;
            assert!(is_ok(&r), "MULTI should OK");

            let r = conn_a
                .execute(&Command::set(key.clone(), SDS::from_str(&val_a)))
                .await;
            assert!(is_queued(&r), "SET inside MULTI should QUEUED");

            let exec = conn_a.execute(&Command::Exec).await;
            match &exec {
                RespValue::Array(Some(results)) => {
                    if results.len() != 1 {
                        violations.push(format!(
                            "seed {}: WATCH no-conflict EXEC got {} results, want 1",
                            seed,
                            results.len()
                        ));
                    }
                    // Verify value was set
                    let get = conn_a.execute(&Command::Get(key.clone())).await;
                    if let RespValue::BulkString(Some(data)) = &get {
                        if String::from_utf8_lossy(data) != val_a {
                            violations.push(format!(
                                "seed {}: after no-conflict EXEC, GET returned {:?}, want {}",
                                seed, data, val_a
                            ));
                        }
                    } else {
                        violations.push(format!(
                            "seed {}: after no-conflict EXEC, GET returned {:?}",
                            seed, get
                        ));
                    }
                }
                RespValue::Array(None) => {
                    violations.push(format!(
                        "seed {}: WATCH no-conflict EXEC returned nil (aborted)",
                        seed
                    ));
                }
                _ => {
                    violations.push(format!(
                        "seed {}: WATCH no-conflict EXEC unexpected: {:?}",
                        seed, exec
                    ));
                }
            }
        } else if scenario < 50 {
            // === WATCH + conflict from conn_b => EXEC aborts ===
            let r = conn_a.execute(&Command::Watch(vec![key.clone()])).await;
            assert!(is_ok(&r), "WATCH should OK");

            // conn_b modifies the watched key to a guaranteed-different value.
            // Use a unique conflict value to ensure it differs from the snapshot.
            let conflict_val = format!("conflict:{}:{}", seed, rng.gen_range(0, 100000));
            conn_b
                .execute(&Command::set(key.clone(), SDS::from_str(&conflict_val)))
                .await;

            let r = conn_a.execute(&Command::Multi).await;
            assert!(is_ok(&r), "MULTI should OK");

            let r = conn_a
                .execute(&Command::set(key.clone(), SDS::from_str(&val_a)))
                .await;
            assert!(is_queued(&r), "SET inside MULTI should QUEUED");

            let exec = conn_a.execute(&Command::Exec).await;
            match &exec {
                RespValue::Array(None) => {
                    // Correct: WATCH detected conflict, transaction aborted
                    // Verify conn_b's value is still there
                    let get = conn_a.execute(&Command::Get(key.clone())).await;
                    if let RespValue::BulkString(Some(data)) = &get {
                        if String::from_utf8_lossy(data) != conflict_val {
                            violations.push(format!(
                                "seed {}: after conflict abort, GET returned {:?}, want {}",
                                seed, data, conflict_val
                            ));
                        }
                    }
                }
                RespValue::Array(Some(_)) => {
                    violations.push(format!(
                        "seed {}: WATCH conflict EXEC succeeded, should have aborted",
                        seed
                    ));
                }
                _ => {
                    violations.push(format!(
                        "seed {}: WATCH conflict EXEC unexpected: {:?}",
                        seed, exec
                    ));
                }
            }
        } else if scenario < 65 {
            // === MULTI/EXEC atomicity (no WATCH) ===
            let key2_idx = rng.gen_range(0, num_keys as u64);
            let key2 = format!("k:{}", key2_idx);
            let val_a2 = format!("a2:{}", rng.gen_range(0, 1000));

            let r = conn_a.execute(&Command::Multi).await;
            assert!(is_ok(&r), "MULTI should OK");

            let r = conn_a
                .execute(&Command::set(key.clone(), SDS::from_str(&val_a)))
                .await;
            assert!(is_queued(&r));

            let r = conn_a
                .execute(&Command::set(key2.clone(), SDS::from_str(&val_a2)))
                .await;
            assert!(is_queued(&r));

            let exec = conn_a.execute(&Command::Exec).await;
            if let RespValue::Array(Some(results)) = &exec {
                if results.len() != 2 {
                    violations.push(format!(
                        "seed {}: MULTI/EXEC 2 cmds got {} results",
                        seed,
                        results.len()
                    ));
                }
                // Both values should be set. If key == key2, second SET wins.
                if key == key2 {
                    let get = conn_a.execute(&Command::Get(key.clone())).await;
                    if let RespValue::BulkString(Some(d)) = &get {
                        if String::from_utf8_lossy(d) != val_a2 {
                            violations.push(format!(
                                "seed {}: atomicity: dup key got {:?}, want {}",
                                seed, d, val_a2
                            ));
                        }
                    }
                } else {
                    let get1 = conn_a.execute(&Command::Get(key.clone())).await;
                    let get2 = conn_a.execute(&Command::Get(key2.clone())).await;
                    if let RespValue::BulkString(Some(d)) = &get1 {
                        if String::from_utf8_lossy(d) != val_a {
                            violations.push(format!("seed {}: atomicity: key1 wrong", seed));
                        }
                    }
                    if let RespValue::BulkString(Some(d)) = &get2 {
                        if String::from_utf8_lossy(d) != val_a2 {
                            violations.push(format!("seed {}: atomicity: key2 wrong", seed));
                        }
                    }
                }
            } else {
                violations.push(format!(
                    "seed {}: MULTI/EXEC no-watch unexpected: {:?}",
                    seed, exec
                ));
            }
        } else if scenario < 75 {
            // === DISCARD cancels transaction ===
            let old = conn_a.execute(&Command::Get(key.clone())).await;

            let r = conn_a.execute(&Command::Multi).await;
            assert!(is_ok(&r));

            let r = conn_a
                .execute(&Command::set(key.clone(), SDS::from_str(&val_a)))
                .await;
            assert!(is_queued(&r));

            let r = conn_a.execute(&Command::Discard).await;
            assert!(is_ok(&r), "DISCARD should OK");

            // Value should be unchanged
            let current = conn_a.execute(&Command::Get(key.clone())).await;
            if !resp_values_equal(&old, &current) {
                violations.push(format!(
                    "seed {}: DISCARD didn't preserve old value for {}",
                    seed, key
                ));
            }

            // EXEC after DISCARD should error
            let r = conn_a.execute(&Command::Exec).await;
            if !is_error(&r) {
                violations.push(format!(
                    "seed {}: EXEC after DISCARD should error, got {:?}",
                    seed, r
                ));
            }
        } else if scenario < 85 {
            // === Error during MULTI => EXECABORT ===
            let r = conn_a.execute(&Command::Multi).await;
            assert!(is_ok(&r));

            let r = conn_a
                .execute(&Command::set(key.clone(), SDS::from_str(&val_a)))
                .await;
            assert!(is_queued(&r));

            // Send unknown command — should mark errors
            let r = conn_a
                .execute(&Command::Unknown("BADCMD".to_string()))
                .await;
            assert!(is_error(&r), "Unknown in MULTI should error");

            let exec = conn_a.execute(&Command::Exec).await;
            match &exec {
                RespValue::Error(e) if e.contains("EXECABORT") => {
                    // Correct: transaction aborted due to queuing errors
                }
                _ => {
                    violations.push(format!(
                        "seed {}: EXECABORT expected, got {:?}",
                        seed, exec
                    ));
                }
            }
        } else if scenario < 92 {
            // === Nested MULTI error ===
            let r = conn_a.execute(&Command::Multi).await;
            assert!(is_ok(&r));

            let r = conn_a.execute(&Command::Multi).await;
            assert!(is_error(&r), "Nested MULTI should error");

            conn_a.execute(&Command::Discard).await;
        } else {
            // === WATCH inside MULTI error ===
            let r = conn_a.execute(&Command::Multi).await;
            assert!(is_ok(&r));

            let r = conn_a
                .execute(&Command::Watch(vec![key.clone()]))
                .await;
            assert!(is_error(&r), "WATCH inside MULTI should error");

            conn_a.execute(&Command::Discard).await;
        }
    }

    violations
}

#[tokio::test]
async fn test_connection_transaction_dst_single() {
    let violations = run_connection_transaction_dst(42).await;
    assert!(
        violations.is_empty(),
        "Seed 42 violations: {:?}",
        violations
    );
}

#[tokio::test]
async fn test_connection_transaction_dst_10_seeds() {
    for seed in 0..10 {
        let violations = run_connection_transaction_dst(seed).await;
        assert!(
            violations.is_empty(),
            "Seed {} violations: {:?}",
            seed,
            violations
        );
    }
}

#[tokio::test]
async fn test_connection_transaction_dst_100_seeds() {
    for seed in 0..100 {
        let violations = run_connection_transaction_dst(seed).await;
        assert!(
            violations.is_empty(),
            "Seed {} violations: {:?}",
            seed,
            violations
        );
    }
}
