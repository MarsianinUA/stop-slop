//! The images the contract set runs, as a user names them. The live job of `stand.yml` keys its
//! cached microsandbox home on this file and `Cargo.lock`: an image added here is one more pull.

pub const ALPINE: &str = "alpine:3.20";
pub const NODE: &str = "node:22-alpine";
/// Its `USER` is 101, not root.
pub const NGINX: &str = "nginxinc/nginx-unprivileged:alpine";
#[cfg(feature = "microsandbox")]
pub const POSTGRES: &str = "postgres:17-alpine";
/// Only the prune case takes it, so pruning it costs no other case a pull.
#[cfg(feature = "microsandbox")]
pub const BUSYBOX: &str = "busybox:1.37";
