//! The network policy of a VM.

use ::microsandbox as msb;

use crate::Outbound;

/// `Off` still reaches the host: the forwarder's upstream is there. Never `default_deny()`.
pub(super) fn policy(outbound: Outbound) -> msb::NetworkPolicy {
    match outbound {
        Outbound::Off => msb::NetworkPolicy::from_profiles([msb::NetworkProfile::Host]),
        Outbound::Public => msb::NetworkPolicy::from_profiles([
            msb::NetworkProfile::Public,
            msb::NetworkProfile::Host,
        ]),
    }
}
