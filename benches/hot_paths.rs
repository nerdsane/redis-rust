//! Hot path benchmarks for profiling-driven optimization.
//!
//! Run with: `cargo bench --bench hot_paths`
//! Compare baselines: `cargo bench --bench hot_paths -- --baseline main`
//!
//! These benchmarks measure the microsecond-level hot paths that
//! dominate Redis performance: set_direct, get_direct, hashing,
//! and RESP encoding.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use redis_sim::redis::{CommandExecutor, RespValue, SDS};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Benchmark CommandExecutor::set_direct - the hot path for SET
fn bench_set_direct(c: &mut Criterion) {
    let mut group = c.benchmark_group("set_direct");
    group.throughput(Throughput::Elements(1));

    // Benchmark with various key/value sizes
    for key_len in [8, 32, 128] {
        let key: String = (0..key_len)
            .map(|i| ((i % 26) as u8 + b'a') as char)
            .collect();
        let value = vec![b'x'; 64];

        group.bench_function(format!("key_len_{}", key_len), |b| {
            let mut executor = CommandExecutor::new();
            b.iter(|| executor.set_direct(black_box(&key), black_box(&value)))
        });
    }

    // Benchmark with various value sizes
    for value_len in [64, 256, 1024] {
        let key = "benchmark_key";
        let value = vec![b'x'; value_len];

        group.bench_function(format!("value_len_{}", value_len), |b| {
            let mut executor = CommandExecutor::new();
            b.iter(|| executor.set_direct(black_box(key), black_box(&value)))
        });
    }

    group.finish();
}

/// Benchmark CommandExecutor::get_direct - the hot path for GET
fn bench_get_direct(c: &mut Criterion) {
    let mut group = c.benchmark_group("get_direct");
    group.throughput(Throughput::Elements(1));

    // Pre-populate keys for GET
    let mut executor = CommandExecutor::new();
    for i in 0..100 {
        let key = format!("key:{}", i);
        let value = format!("value:{}", i);
        executor.set_direct(&key, value.as_bytes());
    }

    // Benchmark existing key (hit)
    group.bench_function("cache_hit", |b| {
        b.iter(|| executor.get_direct(black_box("key:50")))
    });

    // Benchmark missing key (miss)
    group.bench_function("cache_miss", |b| {
        b.iter(|| executor.get_direct(black_box("nonexistent_key")))
    });

    group.finish();
}

/// Benchmark shard hash function
fn bench_hash_key(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_key");
    group.throughput(Throughput::Elements(1));

    let keys = [
        "short",
        "medium_length_key",
        "this_is_a_much_longer_key_that_represents_real_world_patterns",
    ];

    for key in keys {
        group.bench_function(format!("len_{}", key.len()), |b| {
            b.iter(|| {
                let mut hasher = DefaultHasher::new();
                black_box(key).hash(&mut hasher);
                let hash = hasher.finish();
                black_box((hash as usize) % 16)
            })
        });
    }

    // Benchmark bytes hashing (fast path)
    group.bench_function("bytes_32", |b| {
        let key = b"01234567890123456789012345678901";
        b.iter(|| {
            let mut hasher = DefaultHasher::new();
            black_box(key).hash(&mut hasher);
            let hash = hasher.finish();
            black_box((hash as usize) % 16)
        })
    });

    group.finish();
}

/// Benchmark integer encoding (used in RESP responses)
fn bench_encode_integer(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode_integer");
    group.throughput(Throughput::Elements(1));

    // Small integers (most common case)
    group.bench_function("small_i64", |b| {
        b.iter(|| {
            let n: i64 = black_box(42);
            black_box(n.to_string())
        })
    });

    // Medium integers
    group.bench_function("medium_i64", |b| {
        b.iter(|| {
            let n: i64 = black_box(123456);
            black_box(n.to_string())
        })
    });

    // Large integers
    group.bench_function("large_i64", |b| {
        b.iter(|| {
            let n: i64 = black_box(9223372036854775807);
            black_box(n.to_string())
        })
    });

    group.finish();
}

/// Benchmark string allocation patterns (to measure optimization impact)
fn bench_string_alloc(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_alloc");
    group.throughput(Throughput::Elements(1));

    // Benchmark "OK".to_string() (current set_direct behavior)
    group.bench_function("ok_alloc", |b| b.iter(|| black_box("OK".to_string())));

    // Benchmark static string (proposed optimization)
    static OK_STR: &str = "OK";
    group.bench_function("ok_static", |b| b.iter(|| black_box(OK_STR)));

    // Benchmark key.to_string() patterns
    let key = "user:12345:profile";
    group.bench_function("key_clone", |b| {
        b.iter(|| {
            let k = black_box(key).to_string();
            black_box(k)
        })
    });

    // Benchmark double allocation (current set_direct issue)
    group.bench_function("key_double_alloc", |b| {
        b.iter(|| {
            let k1 = black_box(key).to_string();
            let k2 = black_box(key).to_string();
            black_box((k1, k2))
        })
    });

    // Benchmark single alloc + clone (proposed fix)
    group.bench_function("key_single_alloc_clone", |b| {
        b.iter(|| {
            let k = black_box(key).to_string();
            let k2 = k.clone();
            black_box((k, k2))
        })
    });

    group.finish();
}

/// Benchmark bytes copy patterns (for get_direct optimization)
fn bench_bytes_copy(c: &mut Criterion) {
    let mut group = c.benchmark_group("bytes_copy");

    let data = vec![b'x'; 256];

    // Current: .to_vec() (allocates + copies)
    group.bench_function("to_vec_256", |b| {
        b.iter(|| black_box(data.as_slice().to_vec()))
    });

    // Alternative: clone if already vec
    group.bench_function("clone_256", |b| b.iter(|| black_box(data.clone())));

    // Larger data
    let large_data = vec![b'x'; 4096];
    group.bench_function("to_vec_4096", |b| {
        b.iter(|| black_box(large_data.as_slice().to_vec()))
    });

    group.finish();
}

/// Benchmark RespValue creation patterns
fn bench_resp_value(c: &mut Criterion) {
    let mut group = c.benchmark_group("resp_value");
    group.throughput(Throughput::Elements(1));

    // SimpleString allocation (old way with .to_string())
    group.bench_function("simple_string_ok_alloc", |b| {
        b.iter(|| RespValue::simple(black_box("OK").to_string()))
    });

    // SimpleString with static &str (new zero-alloc way)
    group.bench_function("simple_string_ok_static", |b| {
        b.iter(|| RespValue::ok()) // Uses Cow::Borrowed("OK")
    });

    // Integer response (no allocation)
    group.bench_function("integer", |b| b.iter(|| RespValue::Integer(black_box(42))));

    // BulkString with data
    let data = vec![b'x'; 64];
    group.bench_function("bulk_string_64", |b| {
        b.iter(|| RespValue::BulkString(Some(black_box(data.clone()))))
    });

    // BulkString null (nil response)
    group.bench_function("bulk_string_nil", |b| {
        b.iter(|| RespValue::BulkString(None))
    });

    group.finish();
}

/// Benchmark SDS (Simple Dynamic String) operations
fn bench_sds_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("sds");
    group.throughput(Throughput::Elements(1));

    // Create new SDS
    let data = vec![b'x'; 64];
    group.bench_function("new_64", |b| b.iter(|| SDS::new(black_box(data.clone()))));

    // as_bytes (should be zero-cost)
    let sds = SDS::new(data.clone());
    group.bench_function("as_bytes", |b| b.iter(|| black_box(sds.as_bytes())));

    group.finish();
}

criterion_group!(
    benches,
    bench_set_direct,
    bench_get_direct,
    bench_hash_key,
    bench_encode_integer,
    bench_string_alloc,
    bench_bytes_copy,
    bench_resp_value,
    bench_sds_operations,
);

criterion_main!(benches);
