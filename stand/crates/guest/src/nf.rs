//! `table ip stand` and `table ip6 stand`, installed over a netfilter netlink socket of its own:
//! the guest image has no `nft` or `iptables`. One batch builds the rules of the blueprint's
//! Internal formats:
//!
//! ```text
//! table ip stand {
//!   chain out { type nat hook output priority -100
//!     ip daddr <name address> meta l4proto tcp redirect to :<forwarder>    (one per address) }
//!   chain probe { type filter hook output priority 0
//!     meta skgid 65533 ip daddr != 127.0.0.0/8 reject
//!     meta skgid 65533 tcp dport <forwarder> reject } }
//! table ip6 stand {
//!   chain probe { type filter hook output priority 0
//!     meta skgid 65533 ip6 daddr != ::1 reject } }
//! ```
//!
//! The filter chain runs after the nat chain, so a probe that dials a name already has the
//! forwarder's port as its destination there. The names and the forwarder are IPv4 only, so the
//! IPv6 table only keeps a probe from leaving by an IPv6 address, which microsandbox gives the
//! guest whenever the host has an IPv6 route. Numbers `libc` does not name are from
//! `linux/netfilter/nf_tables.h`.

use std::io;
use std::mem;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, TcpStream, UdpSocket};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use guest::PROBE_GID;

const NFNL_SUBSYS_NFTABLES: u16 = libc::NFNL_SUBSYS_NFTABLES as u16;
const NFNL_MSG_BATCH_BEGIN: u16 = libc::NFNL_MSG_BATCH_BEGIN as u16;
const NFNL_MSG_BATCH_END: u16 = libc::NFNL_MSG_BATCH_END as u16;
const NFT_MSG_NEWTABLE: u16 = libc::NFT_MSG_NEWTABLE as u16;
const NFT_MSG_NEWCHAIN: u16 = libc::NFT_MSG_NEWCHAIN as u16;
const NFT_MSG_NEWRULE: u16 = libc::NFT_MSG_NEWRULE as u16;
const NLM_F_REQUEST: u16 = libc::NLM_F_REQUEST as u16;
const NLM_F_ACK: u16 = libc::NLM_F_ACK as u16;
const NLM_F_CREATE: u16 = libc::NLM_F_CREATE as u16;
const NLM_F_EXCL: u16 = libc::NLM_F_EXCL as u16;
const NLM_F_APPEND: u16 = libc::NLM_F_APPEND as u16;
const NLA_F_NESTED: u16 = libc::NLA_F_NESTED as u16;

const NFTA_TABLE_NAME: u16 = 1;
const NFTA_CHAIN_TABLE: u16 = 1;
const NFTA_CHAIN_NAME: u16 = 3;
const NFTA_CHAIN_HOOK: u16 = 4;
const NFTA_CHAIN_POLICY: u16 = 5;
const NFTA_CHAIN_TYPE: u16 = 7;
const NFTA_HOOK_HOOKNUM: u16 = 1;
const NFTA_HOOK_PRIORITY: u16 = 2;
const NFTA_RULE_TABLE: u16 = 1;
const NFTA_RULE_CHAIN: u16 = 2;
const NFTA_RULE_EXPRESSIONS: u16 = 4;
const NFTA_LIST_ELEM: u16 = 1;
const NFTA_EXPR_NAME: u16 = 1;
const NFTA_EXPR_DATA: u16 = 2;
const NFTA_DATA_VALUE: u16 = 1;
const NFTA_META_DREG: u16 = 1;
const NFTA_META_KEY: u16 = 2;
const NFTA_PAYLOAD_DREG: u16 = 1;
const NFTA_PAYLOAD_BASE: u16 = 2;
const NFTA_PAYLOAD_OFFSET: u16 = 3;
const NFTA_PAYLOAD_LEN: u16 = 4;
const NFTA_CMP_SREG: u16 = 1;
const NFTA_CMP_OP: u16 = 2;
const NFTA_CMP_DATA: u16 = 3;
const NFTA_BITWISE_SREG: u16 = 1;
const NFTA_BITWISE_DREG: u16 = 2;
const NFTA_BITWISE_LEN: u16 = 3;
const NFTA_BITWISE_MASK: u16 = 4;
const NFTA_BITWISE_XOR: u16 = 5;
const NFTA_IMMEDIATE_DREG: u16 = 1;
const NFTA_IMMEDIATE_DATA: u16 = 2;
const NFTA_REDIR_REG_PROTO_MIN: u16 = 1;
const NFTA_REJECT_TYPE: u16 = 1;
const NFTA_REJECT_ICMP_CODE: u16 = 2;
/// What `reject` means in `nft`: a TCP `connect` gets `ECONNREFUSED`. Without a code the kernel
/// refuses the whole batch.
const ICMP_PORT_UNREACH: u8 = 3;
const ICMPV6_PORT_UNREACH: u8 = 4;

const IPV4_DADDR: Field = Field {
    base: libc::NFT_PAYLOAD_NETWORK_HEADER as u32,
    offset: 16,
    len: 4,
};
const IPV6_DADDR: Field = Field {
    base: libc::NFT_PAYLOAD_NETWORK_HEADER as u32,
    offset: 24,
    len: 16,
};
const TCP_DPORT: Field = Field {
    base: libc::NFT_PAYLOAD_TRANSPORT_HEADER as u32,
    offset: 2,
    len: 2,
};

/// A second install in the same guest fails rather than leave a rule to a dead port in front.
pub fn install(forwarder_port: u16, addresses: &[Ipv4Addr]) -> io::Result<()> {
    let mut ip = vec![
        (NFT_MSG_NEWTABLE, NLM_F_EXCL, table()),
        (NFT_MSG_NEWCHAIN, 0, chain("out", -100, "nat")),
    ];
    for &address in addresses {
        let rule = redirect(address, forwarder_port);
        ip.push((NFT_MSG_NEWRULE, NLM_F_APPEND, rule));
    }
    ip.push((NFT_MSG_NEWCHAIN, 0, chain("probe", 0, "filter")));
    ip.push((NFT_MSG_NEWRULE, NLM_F_APPEND, probe_off_loopback()));
    ip.push((NFT_MSG_NEWRULE, NLM_F_APPEND, probe_to(forwarder_port)));
    let mut tables = vec![(libc::NFPROTO_IPV4, ip)];
    // A kernel without IPv6 gives a probe no IPv6 socket to leave by.
    if ipv6()? {
        let ip6 = vec![
            (NFT_MSG_NEWTABLE, NLM_F_EXCL, table()),
            (NFT_MSG_NEWCHAIN, 0, chain("probe", 0, "filter")),
            (NFT_MSG_NEWRULE, NLM_F_APPEND, probe_off_loopback6()),
        ];
        tables.push((libc::NFPROTO_IPV6, ip6));
    }
    let bytes = batch(tables);

    let socket = open()?;
    if unsafe { libc::send(socket.as_raw_fd(), bytes.as_ptr().cast(), bytes.len(), 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    read_ack(&socket)
}

/// Where a connection was going before the REDIRECT rule turned it to the forwarder.
pub fn original_dst(stream: &TcpStream) -> Option<SocketAddrV4> {
    let mut address: libc::sockaddr_in = unsafe { mem::zeroed() };
    let mut len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    // SAFETY: the kernel writes at most `len` bytes into `address`.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_IP,
            libc::SO_ORIGINAL_DST,
            (&raw mut address).cast(),
            &mut len,
        )
    };
    (rc == 0).then(|| {
        SocketAddrV4::new(
            Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()),
            u16::from_be(address.sin_port),
        )
    })
}

fn table() -> Attrs {
    Attrs::default().str(NFTA_TABLE_NAME, "stand")
}

fn chain(name: &str, priority: i32, kind: &str) -> Attrs {
    let hook = Attrs::default()
        .be32(NFTA_HOOK_HOOKNUM, libc::NF_INET_LOCAL_OUT as u32)
        .be32(NFTA_HOOK_PRIORITY, priority as u32);
    Attrs::default()
        .str(NFTA_CHAIN_TABLE, "stand")
        .str(NFTA_CHAIN_NAME, name)
        .nest(NFTA_CHAIN_HOOK, hook)
        .be32(NFTA_CHAIN_POLICY, libc::NF_ACCEPT as u32)
        .str(NFTA_CHAIN_TYPE, kind)
}

fn redirect(address: Ipv4Addr, port: u16) -> Attrs {
    let exprs = Attrs::default()
        .elem(load(IPV4_DADDR))
        .elem(cmp(libc::NFT_CMP_EQ, &address.octets()))
        .elem(meta(libc::NFT_META_L4PROTO))
        .elem(cmp(libc::NFT_CMP_EQ, &[libc::IPPROTO_TCP as u8]))
        .elem(expr(
            "immediate",
            Attrs::default()
                .be32(NFTA_IMMEDIATE_DREG, libc::NFT_REG_1 as u32)
                .nest(NFTA_IMMEDIATE_DATA, value(&port.to_be_bytes())),
        ))
        .elem(expr(
            "redir",
            Attrs::default().be32(NFTA_REDIR_REG_PROTO_MIN, libc::NFT_REG_1 as u32),
        ));
    rule("out", exprs)
}

fn probe_off_loopback() -> Attrs {
    let exprs = probe()
        .elem(load(IPV4_DADDR))
        .elem(expr(
            "bitwise",
            Attrs::default()
                .be32(NFTA_BITWISE_SREG, libc::NFT_REG_1 as u32)
                .be32(NFTA_BITWISE_DREG, libc::NFT_REG_1 as u32)
                .be32(NFTA_BITWISE_LEN, 4)
                .nest(NFTA_BITWISE_MASK, value(&[255, 0, 0, 0]))
                .nest(NFTA_BITWISE_XOR, value(&[0, 0, 0, 0])),
        ))
        .elem(cmp(libc::NFT_CMP_NEQ, &[127, 0, 0, 0]))
        .elem(reject(ICMP_PORT_UNREACH));
    rule("probe", exprs)
}

fn probe_to(port: u16) -> Attrs {
    let exprs = probe()
        .elem(meta(libc::NFT_META_L4PROTO))
        .elem(cmp(libc::NFT_CMP_EQ, &[libc::IPPROTO_TCP as u8]))
        .elem(load(TCP_DPORT))
        .elem(cmp(libc::NFT_CMP_EQ, &port.to_be_bytes()))
        .elem(reject(ICMP_PORT_UNREACH));
    rule("probe", exprs)
}

fn probe_off_loopback6() -> Attrs {
    let exprs = probe()
        .elem(load(IPV6_DADDR))
        .elem(cmp(libc::NFT_CMP_NEQ, &Ipv6Addr::LOCALHOST.octets()))
        .elem(reject(ICMPV6_PORT_UNREACH));
    rule("probe", exprs)
}

fn ipv6() -> io::Result<bool> {
    match UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)) {
        Ok(_) => Ok(true),
        Err(error) if error.raw_os_error() == Some(libc::EAFNOSUPPORT) => Ok(false),
        Err(error) => Err(error),
    }
}

/// `meta skgid 65533`; the kernel loads the gid in host byte order.
fn probe() -> Attrs {
    Attrs::default()
        .elem(meta(libc::NFT_META_SKGID))
        .elem(cmp(libc::NFT_CMP_EQ, &PROBE_GID.to_ne_bytes()))
}

fn rule(chain: &str, exprs: Attrs) -> Attrs {
    Attrs::default()
        .str(NFTA_RULE_TABLE, "stand")
        .str(NFTA_RULE_CHAIN, chain)
        .nest(NFTA_RULE_EXPRESSIONS, exprs)
}

fn expr(name: &str, data: Attrs) -> Attrs {
    Attrs::default()
        .str(NFTA_EXPR_NAME, name)
        .nest(NFTA_EXPR_DATA, data)
}

fn meta(key: libc::c_int) -> Attrs {
    expr(
        "meta",
        Attrs::default()
            .be32(NFTA_META_KEY, key as u32)
            .be32(NFTA_META_DREG, libc::NFT_REG_1 as u32),
    )
}

struct Field {
    base: u32,
    offset: u32,
    len: u32,
}

fn load(field: Field) -> Attrs {
    expr(
        "payload",
        Attrs::default()
            .be32(NFTA_PAYLOAD_DREG, libc::NFT_REG_1 as u32)
            .be32(NFTA_PAYLOAD_BASE, field.base)
            .be32(NFTA_PAYLOAD_OFFSET, field.offset)
            .be32(NFTA_PAYLOAD_LEN, field.len),
    )
}

fn cmp(op: libc::c_int, data: &[u8]) -> Attrs {
    expr(
        "cmp",
        Attrs::default()
            .be32(NFTA_CMP_SREG, libc::NFT_REG_1 as u32)
            .be32(NFTA_CMP_OP, op as u32)
            .nest(NFTA_CMP_DATA, value(data)),
    )
}

fn reject(code: u8) -> Attrs {
    expr(
        "reject",
        Attrs::default()
            .be32(NFTA_REJECT_TYPE, libc::NFT_REJECT_ICMP_UNREACH as u32)
            .raw(NFTA_REJECT_ICMP_CODE, &[code]),
    )
}

fn value(bytes: &[u8]) -> Attrs {
    Attrs::default().raw(NFTA_DATA_VALUE, bytes)
}

/// Netlink attributes: length and type in host order, then the payload padded to four bytes.
#[derive(Default)]
struct Attrs(Vec<u8>);

impl Attrs {
    fn raw(mut self, kind: u16, data: &[u8]) -> Self {
        self.0
            .extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
        self.0.extend_from_slice(&kind.to_ne_bytes());
        self.0.extend_from_slice(data);
        while !self.0.len().is_multiple_of(4) {
            self.0.push(0);
        }
        self
    }

    fn str(self, kind: u16, text: &str) -> Self {
        let mut data = text.as_bytes().to_vec();
        data.push(0);
        self.raw(kind, &data)
    }

    /// nf_tables reads its numbers in network byte order.
    fn be32(self, kind: u16, value: u32) -> Self {
        self.raw(kind, &value.to_be_bytes())
    }

    fn nest(self, kind: u16, inner: Attrs) -> Self {
        self.raw(kind | NLA_F_NESTED, &inner.0)
    }

    fn elem(self, expr: Attrs) -> Self {
        self.nest(NFTA_LIST_ELEM, expr)
    }
}

/// The kind of an nf_tables message, its flags beyond the ones every message has, its attributes.
type Message = (u16, u16, Attrs);

/// One transaction of nf_tables messages that create things, by family: the kernel applies all of
/// them or none. It reports a refused message whether or not that one asked for an ack, so only
/// the last asks. Message `n` (from 1) has sequence number `n`, counted across the families.
fn batch(tables: Vec<(libc::c_int, Vec<Message>)>) -> Vec<u8> {
    let mut bytes = Vec::new();
    let control = |bytes: &mut Vec<u8>, kind, seq| {
        let family = libc::AF_UNSPEC as u8;
        let empty = Attrs::default();
        message(
            bytes,
            kind,
            NLM_F_REQUEST,
            seq,
            family,
            NFNL_SUBSYS_NFTABLES,
            &empty,
        );
    };
    control(&mut bytes, NFNL_MSG_BATCH_BEGIN, 0);
    let messages: Vec<_> = tables
        .into_iter()
        .flat_map(|(family, messages)| messages.into_iter().map(move |m| (family as u8, m)))
        .collect();
    let last = messages.len() as u32;
    for (seq, (family, (kind, flags, attrs))) in (1..).zip(messages) {
        let ack = if seq == last { NLM_F_ACK } else { 0 };
        let flags = NLM_F_REQUEST | NLM_F_CREATE | ack | flags;
        let kind = (NFNL_SUBSYS_NFTABLES << 8) | kind;
        message(&mut bytes, kind, flags, seq, family, 0, &attrs);
    }
    control(&mut bytes, NFNL_MSG_BATCH_END, last + 1);
    bytes
}

/// `nlmsghdr`, `nfgenmsg` (its resource id in network order), then the attributes.
fn message(
    out: &mut Vec<u8>,
    kind: u16,
    flags: u16,
    seq: u32,
    family: u8,
    res_id: u16,
    attrs: &Attrs,
) {
    let len = 16 + 4 + attrs.0.len();
    out.extend_from_slice(&(len as u32).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(&flags.to_ne_bytes());
    out.extend_from_slice(&seq.to_ne_bytes());
    out.extend_from_slice(&0u32.to_ne_bytes());
    out.push(family);
    out.push(0);
    out.extend_from_slice(&res_id.to_be_bytes());
    out.extend_from_slice(&attrs.0);
}

fn open() -> io::Result<OwnedFd> {
    let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW, libc::NETLINK_NETFILTER) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a fresh socket nothing else owns.
    let socket = unsafe { OwnedFd::from_raw_fd(fd) };
    // An `nl_pid` of 0 lets the kernel pick the port id.
    let mut address: libc::sockaddr_nl = unsafe { mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    let rc = unsafe {
        libc::bind(
            fd,
            (&raw const address).cast(),
            mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(socket)
}

/// The kernel answers each refused message, then the last one, with an `NLMSG_ERROR` each, one
/// per datagram; the first answer decides. Its code is 0 for the ack, else the negated errno.
fn read_ack(socket: &OwnedFd) -> io::Result<()> {
    let mut buf = [0u8; 8192];
    loop {
        let n = unsafe { libc::recv(socket.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n < 20 || u16::from_ne_bytes([buf[4], buf[5]]) != libc::NLMSG_ERROR as u16 {
            continue;
        }
        let seq = u32::from_ne_bytes(buf[8..12].try_into().unwrap());
        return match i32::from_ne_bytes(buf[16..20].try_into().unwrap()) {
            0 => Ok(()),
            code => {
                let why = io::Error::from_raw_os_error(-code);
                Err(io::Error::other(format!("message {seq}: {why}")))
            }
        };
    }
}
