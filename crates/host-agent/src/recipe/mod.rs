//! Port of Go's `internal/recipe` package, scoped to M6: the shared
//! envelope/source-loading pieces and the `host-recipe.v1` family named in
//! `host_recipe_run`. `runtime-recipe.v1` and `tunnel-recipe.v1` are
//! provider-activation-shaped and deferred to M7.

pub mod host_recipe;
pub mod source;
