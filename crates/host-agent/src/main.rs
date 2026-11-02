//! The Opute Host Agent executable.

use opute_host_agent::{cli, config};
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
