//! Opute Host Agent (Rust) as a library: the agent executable and the provider
//! executables (D26) share it. Behavior is pinned to the Go baseline recorded
//! in `baseline/source-lock.json`; see `openspec/changes/reimplement-host-agent-in-rust`.

// Declared first: the go_err! macro is textually scoped.
#[macro_use]
mod goerr;
mod admission;
mod app;
#[cfg(test)]
mod boundary;
mod builtin;
mod catalog;
pub mod cli;
mod clusterops;
pub mod config;
mod ddl;
mod evidence;
mod goflag;
mod gojson;
mod gotar;
mod host_recipe_mcp;
mod hostarchive;
mod hostartifact;
mod hostcommand;
mod hostconnect;
mod hostexec;
mod hostfiles;
mod hostobs;
mod hostpolicy;
mod hostpublic;
mod hostread;
mod hostservice;
mod hosttool;
mod hostvirt;
mod hostwsl;
mod http1;
mod identity;
mod incus;
mod incusops;
mod llmops;
mod mcpprobe;
mod mcpsdk;
mod oauth;
mod ociops;
mod plan;
mod plan_evidence;
mod plan_mcp;
mod postgresops;
mod servingops;
// M7 core; public provider dispatch stays closed until its adapter is wired.
#[allow(dead_code)]
mod provider;
mod provider_mcp;
pub mod providers;
mod recipe;
mod resource;
mod schema;
#[cfg(test)]
mod seam;
#[cfg(test)]
mod state_fixture;
mod store;
#[cfg(test)]
mod structural_test;
mod tasks;
mod tcprelay;
mod tools;
mod transport;
