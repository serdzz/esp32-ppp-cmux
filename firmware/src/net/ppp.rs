//! PPP runner glue: feeds DLC2 bytes to LCP/IPCP/PAP and translates the
//! negotiated IPv4 lease into an `embassy_net::ConfigV4`.
//!
//! Note on address types: `ppproto::Ipv4Status` returns `core::net::Ipv4Addr`,
//! while `embassy_net::StaticConfigV4` (still re-exporting from smoltcp) wants
//! `smoltcp::wire::Ipv4Address`. We convert via `.octets()` for both the
//! local address and DNS servers.

use embassy_net::{ConfigV4, Ipv4Address, Ipv4Cidr, Stack, StaticConfigV4};
use embassy_net_ppp::{Config, Runner};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use heapless::Vec;

use crate::cmux::channel::DlcChannel;
use crate::cmux::dispatcher::DLC2_PIPE_BYTES;
use crate::config as cfg;
use crate::net::buffered::BufferedDlc;

pub type PppDlc = DlcChannel<CriticalSectionRawMutex, DLC2_PIPE_BYTES>;

#[embassy_executor::task]
pub async fn ppp_task(stack: Stack<'static>, mut runner: Runner<'static>, dlc2: PppDlc) {
    let config = Config {
        username: cfg::GPRS_USER.as_bytes(),
        password: cfg::GPRS_PASS.as_bytes(),
    };

    let mut buffered = BufferedDlc::new(dlc2);
    let result = runner
        .run(&mut buffered, config, |ipv4| {
            let Some(addr) = ipv4.address else {
                log::warn!("IPCP did not provide an IPv4 address");
                return;
            };
            let mut dns_servers: Vec<Ipv4Address, 3> = Vec::new();
            for s in ipv4.dns_servers.iter().flatten() {
                let _ = dns_servers.push(to_smoltcp(*s));
            }
            let smoltcp_addr = to_smoltcp(addr);
            log::info!("PPP up: ip={addr}, dns={:?}", dns_servers.as_slice());
            stack.set_config_v4(ConfigV4::Static(StaticConfigV4 {
                // PPP is point-to-point — prefix 0 mirrors embassy's reference
                // example (no on-link broadcast domain).
                address: Ipv4Cidr::new(smoltcp_addr, 0),
                gateway: None,
                dns_servers,
            }));
        })
        .await;
    log::error!("PPP runner exited: {result:?}");
}

fn to_smoltcp(a: core::net::Ipv4Addr) -> Ipv4Address {
    let o = a.octets();
    Ipv4Address::new(o[0], o[1], o[2], o[3])
}
