//! A static structural test, in the style of Go's own
//! `test/contract/architecture_test.go` (plain source-text scanning, not an
//! AST-level import graph): it fails if any file outside the plan
//! executor's own module and its one legitimate MCP call site constructs a
//! `plan::runner::Runner` -- the only way to invoke the `host-plan.v1` DAG
//! executor, since every node dispatch, retry, compensation, recovery and
//! wait/resume decision lives behind `Runner::run`/`Runner::resume`.
//!
//! This is the Rust side of the M6 single-plan-executor invariant: every
//! recipe family (`host-recipe.v1` today; `runtime-recipe.v1` and
//! `tunnel-recipe.v1` in M7) and every MCP path must run plan execution
//! through this one `Runner`, never a second implementation. A future
//! recipe family that is tempted to hand-roll its own node loop -- instead
//! of expanding into a `plan::schema::Document` and calling
//! `plan_mcp::handle_run_host_plan_with_metadata` the way `host_recipe_mcp`
//! does -- trips this test the moment it writes `Runner {` anywhere else.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The plan executor's own module (construction in its tests is the
    /// implementation testing itself) and `plan_mcp.rs`'s
    /// `spawn_plan_execution`, the one production call site every MCP path
    /// (bare `run_host_plan`, and `host_recipe_mcp`'s
    /// `run_host_local_recipe` through it) funnels through.
    /// `structural_test.rs` is this file -- excluded because its own error
    /// message and doc comments quote the literal pattern being checked
    /// for, not because it constructs one.
    const ALLOWED: &[&str] = &["plan/runner.rs", "plan_mcp.rs", "structural_test.rs"];

    #[test]
    fn only_plan_runner_and_its_one_mcp_caller_construct_a_runner() {
        let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        visit(&src_root, &src_root, &mut offenders);
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "plan::runner::Runner {{ ... }} constructed outside the allowed call \
             sites {ALLOWED:?} -- every plan execution path must go through the \
             single executor, not a second implementation: {offenders:?}"
        );
    }

    fn visit(root: &Path, dir: &Path, offenders: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, offenders);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let relative = relative_slash_path(root, &path);
            if ALLOWED.contains(&relative.as_str()) {
                continue;
            }
            let contents = fs::read_to_string(&path).unwrap();
            if contents.contains("Runner {") {
                offenders.push(relative);
            }
        }
    }

    fn relative_slash_path(root: &Path, path: &Path) -> String {
        let relative: PathBuf = path.strip_prefix(root).unwrap().to_path_buf();
        relative.to_string_lossy().replace('\\', "/")
    }
}
