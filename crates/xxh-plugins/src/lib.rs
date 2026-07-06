//! xxh-plugins — plugin registry, resolver, isolation, and package sources.
//!
//! `trait PackageSource` (T032) hides how a package is obtained: git, a local
//! path, or (⭐ behind the `nix-source` feature) nixpkgs — Принцип IX. The
//! registry (T035) stores packages content-addressed; the resolver (T036)
//! orders them deterministically; isolation (T037) runs lifecycle hooks in a
//! separate process so a broken plugin cannot take the session down.

pub use xxh_plugin_api::{Manifest, PluginError};

pub mod isolation;
pub mod registry;
pub mod resolver;
pub mod source;
pub mod sources;
