// `forbid`, not `deny`: a check.rs cannot lift the clippy.toml bans with its own `allow`.
#![forbid(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    unsafe_code
)]

include!(concat!(env!("OUT_DIR"), "/registry.rs"));
