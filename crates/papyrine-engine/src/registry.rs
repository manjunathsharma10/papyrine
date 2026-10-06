//! The engine's command registry: the built-in organise commands of `papyrine-ops` plus the
//! registration functions of the feature crates.
//!
//! `papyrine-annotate` and `papyrine-fill` expose `fn register(&mut CommandRegistry)`; they are
//! wired in here as they land (a path dependency on a missing crate would break the whole
//! workspace, so the calls are added with the crates).

use papyrine_ops::CommandRegistry;

pub fn default_registry() -> CommandRegistry {
    let r = CommandRegistry::with_builtin();
    // papyrine_annotate::register(&mut r);
    // papyrine_fill::register(&mut r);
    r
}
