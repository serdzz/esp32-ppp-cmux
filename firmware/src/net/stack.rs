//! `embassy-net` stack init + the small handle the app layer uses to open
//! sockets.

use embassy_executor::Spawner;
use embassy_net::{Config, Stack, StackResources};
use esp_hal::rng::Rng;
use static_cell::StaticCell;

use crate::net::ppp::{ppp_task, PppDlc};

/// Number of concurrent embassy-net sockets. One TCP for MQTT/TLS plus DNS
/// + a spare = 3 is enough for v1.
pub const STACK_SOCKETS: usize = 3;

/// PPP IO buffer slots — passed to `embassy_net_ppp::State<RX, TX>`. With
/// MTU = 1500 and PPP HDLC overhead, 4 each is comfortable.
pub const PPP_RX_FRAMES: usize = 4;
pub const PPP_TX_FRAMES: usize = 4;

pub struct Net {
    pub stack: Stack<'static>,
}

/// Spawn the PPP runner and the embassy-net stack background task. Returns
/// once both are running. The IP isn't up yet — caller awaits it via
/// `stack.wait_config_up().await`.
///
/// `Rng::new()` no longer needs a peripheral handle in esp-hal 1.x; the
/// TRNG is initialised by `esp_hal::init` and exposed as a global reader.
pub fn start(spawner: Spawner, dlc2: PppDlc) -> Net {
    static PPP_STATE: StaticCell<embassy_net_ppp::State<PPP_RX_FRAMES, PPP_TX_FRAMES>> =
        StaticCell::new();
    let ppp_state =
        PPP_STATE.init(embassy_net_ppp::State::<PPP_RX_FRAMES, PPP_TX_FRAMES>::new());
    let (device, ppp_runner) = embassy_net_ppp::new(ppp_state);

    let mut rng = Rng::new();
    let mut seed_bytes = [0u8; 8];
    rng.read(&mut seed_bytes);
    let seed = u64::from_le_bytes(seed_bytes);

    static RESOURCES: StaticCell<StackResources<STACK_SOCKETS>> = StaticCell::new();
    let resources = RESOURCES.init(StackResources::new());

    // No initial v4 config — PPP will hand one over once IPCP completes.
    let (stack, net_runner) = embassy_net::new(device, Config::default(), resources, seed);

    spawner.spawn(net_task(net_runner).unwrap());
    spawner.spawn(ppp_task(stack, ppp_runner, dlc2).unwrap());

    Net { stack }
}

#[embassy_executor::task]
async fn net_task(
    mut runner: embassy_net::Runner<'static, embassy_net_ppp::Device<'static>>,
) -> ! {
    runner.run().await
}
