# esp32-ppp-cmux

Rust + Embassy firmware for the **TTGO T-Call v1.x** board (ESP32 + on-board SIM800L). Brings up GPRS through SIM800L, multiplexes the single modem UART with home-grown **3GPP TS 27.010 basic-mode CMUX** so AT status (DLC1) and PPP IP traffic (DLC2) flow in parallel, plugs PPP into an `embassy-net` stack, opens TLS to a configured MQTT broker.

> **Project status (v1)**: full pipeline compiles end-to-end for `xtensa-esp32-none-elf`. CMUX framing has 21 host-side tests (incl. 3 proptest fuzzers, ~3 000 cases). MQTT framing wiring is intentionally a TODO — the v1 app only does DNS → TCP → TLS handshake → smoke-test write. See `app/mqtt.rs`.

## Layout

```
.
├── cmux-core/       # no_std lib: 27.010 frame encoder/decoder, FCS, FSM. Host-testable.
└── firmware/        # bin crate: target xtensa-esp32-none-elf
    └── src/
        ├── board.rs          # T-Call pin map
        ├── power/{ip5306,sim800}.rs   # PMIC boost-keep-on + modem power sequence
        ├── cmux/              # dispatcher + TX task + DlcChannel embedded_io_async handles
        ├── modem/bringup.rs   # AT init → CMUX entry → AT+CGDATA PPP
        ├── net/{ppp,stack,buffered,tls}.rs  # embassy-net-ppp + stack + TLS over TcpSocket
        └── app/{mqtt,status}.rs  # smoke-test MQTT/TLS + periodic CSQ/CREG
```

The **architecture deep-dive** is in `CLAUDE.md`.

## Hardware

TTGO T-Call v1.x. Pinout is fixed by the board:

| Signal | ESP32 GPIO |
|---|---|
| SIM800L TX → ESP32 RX (UART1) | 27 |
| SIM800L RX ← ESP32 TX (UART1) | 26 |
| SIM800L RST | 5 |
| SIM800L PWKEY | 4 |
| SIM800L POWER_ON gate | 23 |
| IP5306 PMIC SDA / SCL (I²C0) | 21 / 22 |

UART is 115 200 8N1 with **no HW flow control** (RTS/CTS not routed). SIM800L is **2G/GPRS only** — verify your operator still has 2G in the target region.

## Toolchain

```sh
cargo install espup espflash
espup install                    # installs the xtensa rustc fork + esp-clang + xtensa-esp-elf-gcc
. ~/export-esp.sh                # add to your shell rc; needed before xtensa builds
```

`ldproxy` is **not** required — esp-hal 1.x links directly via `xtensa-esp-elf-gcc` from espup.

## Configure

```sh
cp .env.example .env
$EDITOR .env
```

Required vars (build.rs panics-with-warning if empty, runtime fails clearly):
- `APN`, `MQTT_HOST`, `MQTT_PORT`, `MQTT_CLIENT_ID`

Optional:
- `GPRS_USER`, `GPRS_PASS`, `MQTT_USER`, `MQTT_PASS`, `SIM_PIN`, `MQTT_DNS`
- `MQTT_CA_PEM` — path (relative to `firmware/`) to a PEM root CA. If absent, build emits a warning and the TLS handshake will fail at runtime (use this to test up to TCP).

## Build & flash

Host-side CMUX tests:

```sh
cargo test -p cmux-core --target aarch64-apple-darwin
```

Firmware build (must run from inside `firmware/` so its `.cargo/config.toml` applies the xtensa target):

```sh
. ~/export-esp.sh
cd firmware
cargo build --release
cargo run   --release   # espflash flash --monitor (configured runner)
```

## Expected boot log (golden path)

```
INFO  - esp32-ppp-cmux booting
INFO  - IP5306 boost-keep-on configured
INFO  - SIM800 PWKEY released; awaiting RDY URC on modem UART
DEBUG - heartbeat #0
... (modem boot URCs RDY, +CFUN: 1, +CPIN: READY, Call Ready, SMS Ready)
INFO  - registered to network
INFO  - modem entered CMUX mode
INFO  - CMUX DLC0 open
INFO  - CMUX DLC1 open
INFO  - CMUX DLC2 open
INFO  - PPP CONNECT received on DLC2
INFO  - net stack started, waiting for IPCP
INFO  - PPP up: ip=10.x.x.x, dns=[...]
INFO  - mqtt task: IP up, attempting connection loop
INFO  - resolved <broker> -> <ip>
INFO  - TCP up to <ip>:8883
INFO  - TLS handshake OK to <broker>:8883
INFO  - status [AT+CSQ]: +CSQ: 18,99
INFO  - status [AT+CREG?]: +CREG: 2,1
```

## Things that go wrong first (and where to look)

- **Modem stays silent** → IP5306 not initialised. Check `power::ip5306` log line, double-check I²C wiring on GPIO 21/22.
- **`+CREG` stuck at 0,2 (searching)** → no 2G coverage in your region or wrong APN.
- **CMUX SABM timeout** on DLC0 right after `AT+CMUX=0` → the post-OK UART drain in `main.rs` is the prime suspect; the modem may have buffered URCs that look like garbage to the dispatcher.
- **PPP runner exits immediately** → IPCP failed. Verify GPRS attach (`AT+CGATT?` returns 1) and that `AT+CGDATA="PPP",1` returned `CONNECT` (look in the log).
- **TLS handshake errors** → CA cert mismatch. The default config falls back to no verification only when `MQTT_CA_PEM` is empty; set it to a real PEM to validate.

## v1 explicitly out of scope

- 27.010 advanced mode, MSC software flow control.
- PPP auto-reconnect (v1: panic + reset on bring-up failure).
- Full MQTT publish/subscribe framing (rust-mqtt 0.4 `Client<...>` API wiring is the v2 ticket).
- SMS, voice, GNSS.
- DTR-driven low-power (`AT+CSCLK`) — DTR is not routed on T-Call.
- OTA, multi-PDP, CHAP authentication. v1 uses PAP only.

## License

MIT OR Apache-2.0.
