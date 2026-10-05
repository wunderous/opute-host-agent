//! Port of Go's `internal/plan` package: the `host-plan.v1` declarative DAG
//! executor. Pure and MCP-free, same as the Go original -- callers supply a
//! dispatcher closure and get back run state.

pub mod assert;
pub mod graph;
pub mod interpolate;
pub mod runner;
pub mod schema;
