# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Greenfield Rust + Embassy firmware for the **TTGO T-Call v1.x** board (classic ESP32 Xtensa LX6 + on-board SIM800L 2G/GPRS modem). The firmware:

1. Powers up the SIM800L through the IP5306 PMIC (I²C boot dance — without it the modem rail collapses).
2. Brings up GPRS, switches the single UART into **3GPP TS 27.010 basic-mode CMUX**.
3. Multiplexes the UART into 3 virtual channels: DLC0 (control), DLC1 (AT commands via `atat`), DLC2 (PPP carrying IP traffic).
4. Runs LCP/IPCP through `embassy-net-ppp` on DLC2, plugged into an `embassy-net` stack.
5. Connects to an MQTT broker over TLS (`embedded-tls` + `rust-mqtt`) and runs publish/subscribe + a periodic AT status loop in parallel.

Full architecture and rationale live in the session plan file under `~/.claude/plans/` (machine-local; ask the user for the active filename if you need to consult it).

**Commit/PR style**: messages, titles, and identifiers in English; chat with the user can be in Russian.

## Workspace layout

```
.
├── cmux-core/      # no_std lib, host-testable (#![cfg_attr(not(test), no_std)])
│   └── src/        # frame, fcs, address, control, state
└── firmware/       # bin crate, target xtensa-esp32-none-elf
    ├── build.rs    # APN/MQTT_*/SIM_PIN/MQTT_CA_PEM → rustc-env from .env
    └── src/        # main, board, config, logger, power/, cmux/, modem/, net/, app/
```

`cmux-core` is the only piece worth host-testing (framing bugs are nasty). Everything else is firmware-only.

**Hard invariant in `cmux-core`**: only single-byte length encoding (EA=1) is supported, capping `MAX_INFO_LEN` at 127. The decoder explicitly rejects two-byte length frames (`DecodeError::TwoByteLengthUnsupported`). Anyone widening the MTU must touch the encoder, decoder state machine, FCS coverage, *and* the firmware-side pipe sizing in lockstep.

## Toolchain

Uses the **`esp` custom rustc fork** installed by `espup` (Xtensa LX6 has no upstream rustc support). It lives at `~/.rustup/toolchains/esp/`. Before any xtensa build:

```sh
. ~/export-esp.sh   # adds xtensa-esp-elf gcc + esp-clang to PATH
```

Required tools: `espup`, `espflash` (already installed in `~/.cargo/bin/`). `ldproxy` is **not** used — esp-hal 1.x links directly via the esp toolchain's `xtensa-esp-elf-gcc` with `-Wl,-Tlinkall.x`.

**Quirk**: `rust-toolchain.toml` for a custom toolchain **cannot** include a `components` field — rustup errors with "toolchain options are ignored for a custom toolchain". `rust-src` ships built-in.

The dependency stack uses the new `esp-rtos` crate (with `embassy` feature) instead of the older `esp-hal-embassy`. Use `#[esp_rtos::main]`, not `#[esp_hal_embassy::main]`.

## Common commands

**Host tests (cmux-core)** — run from anywhere in the workspace; force the host triple to override `firmware/.cargo/config.toml`:

```sh
cargo test -p cmux-core --target aarch64-apple-darwin
```

**Firmware build** — must run from inside `firmware/` so its `.cargo/config.toml` (xtensa target, ldproxy, build-std) applies:

```sh
. ~/export-esp.sh
cd firmware && cargo build --release
```

Building from the workspace root with `cargo build -p esp32-ppp-cmux-fw` will compile for the host and fail (linker/section errors from xtensa-only crates).

**Flash + serial monitor**:

```sh
cd firmware && cargo run --release   # uses espflash via .cargo/config.toml runner
```

**Single test**:

```sh
cargo test -p cmux-core --target aarch64-apple-darwin -- frame::tests::sabm_round_trip
```

**Lint**:

```sh
cargo clippy -p cmux-core --target aarch64-apple-darwin -- -D warnings
. ~/export-esp.sh && cd firmware && cargo clippy --release -- -D warnings
```

## Dependency convention

Common versions live in `[workspace.dependencies]` at the workspace root. Per-crate `Cargo.toml` should always reference them via `{ workspace = true, features = [...] }` instead of pinning a separate version — otherwise it's easy to silently end up with two `embassy-time` (or `embassy-sync`) majors in the dep graph, which breaks type identity for `Pipe`/`Channel` shared across modules.

## Known build pitfalls

- **`esp-println` requires exactly one of `jtag-serial | uart | auto | no-op`** as a feature. We use `auto` (UART when no JTAG connected, JTAG otherwise). Forgetting it → build.rs panic with that exact message.
- **`esp-backtrace` 0.19 dropped `exception-handler`** — only `panic-handler`, `println`, `colors`, plus chip features.
- **Workspace-root `cargo build -p firmware`** picks up the workspace `.cargo/config.toml` (no target) and tries to build xtensa crates for the host → linker errors about Mach-O sections and `xtensa_lx`. Always `cd firmware` first.
- **`rust-mqtt` won't compile without `v5` feature** (≤ 0.5.1 has v5-only code paths outside the feature gate). Use `features = ["v5", "alloc", "log"]`. Also: `rust-mqtt 0.5` requires rustc ≥ 1.87 — the esp xtensa fork is older, so we pin 0.4.x.
- **embassy-executor 0.10 changed `#[task]` return type** to `Result<SpawnToken, SpawnError>`. Spawn pattern is `spawner.spawn(my_task().unwrap())`, not `must_spawn(my_task())`.

## Configuration

`firmware/build.rs` reads `.env` (firmware/ first, then workspace root) via `dotenvy` and re-exports values as `cargo:rustc-env=...`. Required: `APN`, `MQTT_HOST`, `MQTT_PORT`, `MQTT_CLIENT_ID`. Optional: `GPRS_USER`, `GPRS_PASS`, `MQTT_USER`, `MQTT_PASS`, `SIM_PIN`, `MQTT_DNS`. CA cert path is `MQTT_CA_PEM` (relative to `firmware/`); build.rs copies it into `OUT_DIR` so `include_bytes!` always resolves — missing cert → empty stub + cargo warning, not a build failure.

Bootstrap a local config:

```sh
cp .env.example .env   # then edit values
```

## Architecture (big picture)

```
SIM800L ── UART (115200,8N1, no flow ctrl) ── esp-hal Uart
                                                  │
                              cmux::dispatcher (RX) + cmux::tx (TX)
                                                  │
                       per-DLC Pipe<512/4096>   Channel<TxReq> + Pool<[u8;128]>
                                  │                   │
                  ┌───────────────┼──────────────────┘
                  ▼               ▼
          DlcChannel(1)     DlcChannel(2)             ← embedded_io_async
                  │               │
                  ▼               ▼
            atat client   embassy-net-ppp Runner
                  │               │
              URC + status   embassy-net Stack (IPv4, TCP, DNS)
                                  │
                          embedded-tls (TLS 1.2/1.3)
                                  │
                            rust-mqtt client
```

Single ownership rules to preserve:
- **`cmux::tx` is the sole owner of the UART TX half** post-CMUX entry. All upstream writers (atat, PPP runner) push `TxReq` messages into one `Channel`; the TX task serializes into 27.010 frames.
- **`cmux::dispatcher` owns the UART RX half**. Per-DLC `embassy_sync::pipe::Pipe` is the byte-stream primitive (the `Read` contract `atat` and `embassy-net-ppp` need); not `Channel<T>`.
- **`bringup` task drops the raw `Uart` before tasks consume the split halves** to enforce phase transition (Raw AT → CMUX) statically.

Critical timing/ordering gotchas (don't lose these):
- After `AT+CMUX=0,...` returns OK, the modem switches into mux mode immediately. **Drain UART for ~50 ms, then start dispatcher, then send SABM(0)** — any non-frame byte after OK breaks mux.
- `AT+CGDATA="PPP",1` is sent on DLC2 **before** PPP handshake; its `CONNECT\r\n` response arrives on DLC2 itself. Parse that response in `bringup` before handing DLC2 to `embassy-net-ppp::Runner`.
- IP5306 init must run **before** powering the modem (POWER_ON HIGH → PWKEY 1100 ms LOW → wait `RDY` URC). `BOOST_KEEP_ON` (write `0x37` to reg `0x00`) is mandatory: without it the rail collapses on light load and the modem brown-out resets.
- No HW flow control on T-Call (RTS/CTS not routed). DLC2 RX pipe must be ≥ 4 KB to absorb PPP bursts.

## Hardware pin map (TTGO T-Call v1.x)

| Signal | ESP32 GPIO |
|---|---|
| SIM800L TX → ESP32 RX (UART1) | 26 |
| SIM800L RX ← ESP32 TX (UART1) | 27 |
| SIM800L RST | 5 |
| SIM800L PWKEY | 4 |
| SIM800L POWER_ON gate | 23 |
| IP5306 PMIC SDA (I²C0) | 21 |
| IP5306 PMIC SCL (I²C0) | 22 |

Constants live in `firmware/src/board.rs`.

## Memory & log levels

- `ESP_LOG` env var sets the runtime log level (read by `esp_println::logger::init_logger_from_env()`). Default `info` from `.cargo/config.toml`.
- Embassy task arena and per-DLC pipe sizes are intentional — see plan §3 for sizing rationale.

## v1 explicitly out of scope

Don't add (without discussion): 27.010 advanced mode, MSC software flow control, PPP auto-reconnect, SMS/voice/GNSS, low-power modes (DTR pin not routed on this board), OTA, multi-PDP, CHAP. v1 uses PAP only and panics + resets on bring-up failure.
