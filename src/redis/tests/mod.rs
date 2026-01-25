//! Redis module tests
//!
//! Split from the original monolithic tests.rs for better organization
//! and to comply with 500-line file limit.

mod command_parser_tests;
mod list_command_tests;
mod resp_parser_tests;
mod scan_tests;
mod set_option_tests;
mod sorted_set_command_tests;
mod transaction_tests;

// Lua scripting tests (feature-gated)
#[cfg(feature = "lua")]
mod lua_basic_tests;
#[cfg(feature = "lua")]
mod lua_command_tests;
#[cfg(feature = "lua")]
mod lua_redis_call_tests;
