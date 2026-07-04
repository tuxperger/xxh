//! xxh-plugins — plugin registry, resolver, isolation, and package sources.
//!
//! Skeleton for Phase 1. `trait PackageSource` (T032) and the git/local/⭐nix
//! providers (T033/T034/T050) land in US4/US7. See contracts/plugin-source-trait.md.

pub use xxh_plugin_api::{Manifest, PluginError};

pub mod resolver;

// ⭐ Optional Nix provider is compiled only behind the `nix-source` feature so its
// absence cannot affect the rest of the tool (Принцип IX).
#[cfg(feature = "nix-source")]
pub mod sources {
    //! Provider implementations land here (T033/T034/T050).
}
