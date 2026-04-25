//! MQTT-over-TLS application task.
//!
//! v1 scope: TCP connect → TLS handshake → write a small payload → loop.
//! Full MQTT 3.1.1/5 framing (rust-mqtt) wiring is intentionally deferred:
//! rust-mqtt 0.4.x has a `Client<...>` builder API that doesn't match the
//! 0.3/0.5 reference examples, and the right call site emerges naturally
//! once we can validate it against a real broker. See TODO(v2) below.

use core::net::IpAddr;
use core::str::FromStr;

use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, Stack};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write as _;

use crate::config;
use crate::net::tls::{open as tls_open, EspRng};

const SOCKET_BUF: usize = 4096;
const TLS_BUF: usize = 18 * 1024;

#[embassy_executor::task]
pub async fn mqtt_task(stack: Stack<'static>) {
    log::info!("mqtt task: waiting for IPv4 config");
    stack.wait_config_up().await;
    log::info!("mqtt task: IP up, attempting connection loop");

    loop {
        if let Err(e) = run_session(stack).await {
            log::error!("mqtt session ended: {e:?}; reconnect in 10s");
        }
        Timer::after(Duration::from_secs(10)).await;
    }
}

#[derive(Debug)]
enum SessionError {
    Dns,
    Tcp,
    Tls,
}

async fn run_session(stack: Stack<'static>) -> Result<(), SessionError> {
    let host = config::MQTT_HOST;
    let port = config::mqtt_port();

    // Resolve host (accept literal IP or DNS A record).
    let addr: IpAddress = if let Ok(ip) = IpAddr::from_str(host) {
        match ip {
            IpAddr::V4(v4) => IpAddress::v4(
                v4.octets()[0],
                v4.octets()[1],
                v4.octets()[2],
                v4.octets()[3],
            ),
            IpAddr::V6(_) => return Err(SessionError::Dns),
        }
    } else {
        let v4s = stack.dns_query(host, DnsQueryType::A).await.map_err(|e| {
            log::error!("DNS failed for {host}: {e:?}");
            SessionError::Dns
        })?;
        *v4s.first().ok_or_else(|| {
            log::error!("DNS returned no A records for {host}");
            SessionError::Dns
        })?
    };
    log::info!("resolved {host} -> {addr}");

    let mut rx = [0u8; SOCKET_BUF];
    let mut tx = [0u8; SOCKET_BUF];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
    socket.set_timeout(Some(Duration::from_secs(15)));
    socket.connect((addr, port)).await.map_err(|e| {
        log::error!("TCP connect to {addr}:{port} failed: {e:?}");
        SessionError::Tcp
    })?;
    log::info!("TCP up to {addr}:{port}");

    let mut tls_rx = [0u8; TLS_BUF];
    let mut tls_tx = [0u8; TLS_BUF];
    let mut rng = EspRng(esp_hal::rng::Rng::new());

    let mut tls = tls_open(socket, &mut tls_rx, &mut tls_tx, host, &mut rng)
        .await
        .map_err(|e| {
            log::error!("TLS handshake failed: {e:?}");
            SessionError::Tls
        })?;
    log::info!("TLS handshake OK to {host}:{port}");

    // TODO(v2): wire rust-mqtt client here. The 0.4.x API is `Client<...>`
    // with `connect()` / `publish()` / `subscribe()` builder methods — see
    // src/client/mod.rs for the exact shape. For v1 we just demonstrate the
    // TLS link is alive by sending an HTTP-shaped probe and idling.
    let probe = b"PINGREQ\r\n";
    if let Err(e) = tls.write_all(probe).await {
        log::warn!("TLS write_all failed: {e:?}");
    }
    log::info!("smoke-test payload sent; idling");

    loop {
        Timer::after(Duration::from_secs(60)).await;
        log::debug!("mqtt heartbeat tick (no MQTT framing yet — v2)");
    }
}
