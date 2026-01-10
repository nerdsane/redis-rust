// Clippy configuration: allow some stylistic lints to focus on correctness
// These are all pedantic/style lints that don't affect correctness
#![allow(clippy::manual_is_multiple_of)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_else_if)]
#![allow(clippy::type_complexity)]
#![allow(clippy::needless_lifetimes)]
#![allow(clippy::or_fun_call)]
#![allow(clippy::inherent_to_string)]
#![allow(clippy::question_mark)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::useless_format)]
#![allow(clippy::new_without_default)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::map_flatten)]
#![allow(clippy::option_map_or_none)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::redundant_closure)]
#![allow(clippy::len_zero)]
#![allow(clippy::bool_comparison)]
#![allow(clippy::manual_map)]
#![allow(clippy::single_match)]
#![allow(clippy::redundant_pattern_matching)]
#![allow(clippy::for_kv_map)]
#![allow(clippy::expect_fun_call)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::match_like_matches_macro)]
#![allow(clippy::manual_range_contains)]
#![allow(clippy::wrong_self_convention)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::from_over_into)]
#![allow(clippy::result_unit_err)]
#![allow(clippy::clone_on_copy)]
#![allow(clippy::derive_ord_xor_partial_ord)]
#![allow(clippy::unwrap_or_default)]
#![allow(clippy::io_other_error)]
#![allow(clippy::only_used_in_recursion)]
#![allow(clippy::iter_cloned_collect)]
#![allow(clippy::match_single_binding)]
#![allow(clippy::derivable_impls)]
#![allow(clippy::explicit_counter_loop)]
#![allow(clippy::needless_borrows_for_generic_args)]
#![allow(clippy::manual_strip)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::manual_find)]
#![allow(clippy::unnecessary_map_or)]
// Allow dead code for utility functions that may be used later
#![allow(dead_code)]
// Allow unused comparisons in debug assertions
#![allow(unused_comparisons)]
// Allow unused must_use in tests
#![allow(unused_must_use)]
// Allow unused variables in tests
#![allow(unused_variables)]

pub mod buggify;
pub mod io;
pub mod metrics;
pub mod production;
pub mod redis;
pub mod replication;
pub mod security;
pub mod simulator;
pub mod streaming;

// Observability: feature-gated Datadog integration
#[cfg(feature = "datadog")]
pub mod observability;

#[cfg(not(feature = "datadog"))]
#[path = "observability_noop.rs"]
pub mod observability;

pub use redis::{RedisClient, RedisServer, RespParser, Value};
pub use simulator::{Host, NetworkEvent, Simulation, SimulationConfig};
