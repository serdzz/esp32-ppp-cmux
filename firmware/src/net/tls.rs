//! TLS 1.2/1.3 wrapper over an `embassy_net::tcp::TcpSocket`.
//!
//! Thin adapter so the MQTT layer just sees `embedded_io_async`
//! Read+Write that happens to encrypt. The CA root cert is baked in at
//! compile time from `MQTT_CA_PEM` (build.rs).
//!
//! The esp-hal RNG implements `rand_core_06::RngCore` but not `CryptoRng`
//! (the marker trait). We wrap it in [`EspRng`] which adds the CryptoRng
//! claim — the on-chip TRNG is HW-backed and acceptable for TLS in
//! production after esp-hal-rng has been seeded post-boot per its docs.

use embassy_net::tcp::TcpSocket;
use embedded_tls::{
    Aes128GcmSha256, Certificate, TlsConfig, TlsConnection, TlsContext, UnsecureProvider,
};
use rand_core::{CryptoRng, RngCore};

use crate::config;

/// Newtype around `esp_hal::rng::Rng` to add the `CryptoRng` marker.
pub struct EspRng(pub esp_hal::rng::Rng);

impl RngCore for EspRng {
    fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }
    fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.fill_bytes(dest);
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.0.try_fill_bytes(dest)
    }
}

impl CryptoRng for EspRng {}

/// Open a TLS session over an already-connected `TcpSocket`.
///
/// Buffers must outlive the connection — typically `static_cell` slabs at
/// the call site.
pub async fn open<'a, 'b>(
    socket: TcpSocket<'b>,
    record_read_buf: &'a mut [u8],
    record_write_buf: &'a mut [u8],
    server_name: &'a str,
    rng: &mut EspRng,
) -> Result<TlsConnection<'a, TcpSocket<'b>, Aes128GcmSha256>, embedded_tls::TlsError>
where
    'b: 'a,
{
    let mut config = TlsConfig::new().with_server_name(server_name);
    if !config::MQTT_CA_PEM.is_empty() {
        config = config.with_ca(Certificate::X509(config::MQTT_CA_PEM));
    }

    let mut conn: TlsConnection<'a, _, Aes128GcmSha256> =
        TlsConnection::new(socket, record_read_buf, record_write_buf);

    // `UnsecureProvider` is a `CryptoProvider` that uses `NoVerify` under
    // the hood. Type inference picks it up from the `TlsContext`.
    conn.open(TlsContext::new(
        &config,
        UnsecureProvider::new::<Aes128GcmSha256>(rng),
    ))
    .await?;

    Ok(conn)
}
