//! Opute Host Agent (Rust). Behavior is pinned to the Go baseline recorded in
//! `baseline/source-lock.json`; see `openspec/changes/reimplement-host-agent-in-rust`.

// Declared first: the go_err! macro is textually scoped.
#[macro_use]
mod goerr;
mod admission;
mod app;
mod catalog;
mod cli;
mod config;
mod ddl;
mod evidence;
mod goflag;
mod gojson;
mod host_recipe_mcp;
mod hostobs;
mod hostread;
mod http1;
mod identity;
mod incus;
mod mcpsdk;
mod oauth;
mod plan;
mod plan_evidence;
mod plan_mcp;
mod recipe;
mod resource;
mod schema;
#[cfg(test)]
mod state_fixture;
mod store;
#[cfg(test)]
mod structural_test;
mod tasks;
mod tools;
mod transport;

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr();
    let result = {
        let mut io = cli::Io {
            stdout: &mut stdout,
            stderr: &mut stderr,
        };
        cli::run(&args, config::Env::from_process(), &mut io)
    };
    let _ = stdout.flush();
    if let Err(err) = result {
        // Go: fmt.Fprintln(os.Stderr, err); os.Exit(1)
        let _ = writeln!(stderr, "{err}");
        std::process::exit(1);
    }
}
