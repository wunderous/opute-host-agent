//! Port of Go's `internal/recipe` package: the shared envelope/source-loading
//! pieces, the `host-recipe.v1` family named in `host_recipe_run`, and the
//! provider-activation-shaped `runtime-recipe.v1` and `tunnel-recipe.v1`
//! families. Every family expands to one host plan for the single executor.

pub mod host_recipe;
pub mod runtime;
pub mod source;
