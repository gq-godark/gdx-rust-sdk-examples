# GoDark Rust SDK

Encrypted Rust client for the GoDark DEX.

## Quickstart

**Standalone clone (recommended):** `gdx-proto` is tracked as a git submodule
pinned to a specific commit on `v1/devnet` (see `.gitmodules`), matching the
python / java / cpp SDKs. Clone recursively so the submodule populates:

```bash
git clone --recurse-submodules <repo-url> gdx-rust-sdk
cd gdx-rust-sdk
cargo build --all-targets
cargo test
```

If you already cloned without `--recurse-submodules`, run
`git submodule update --init --recursive` once to populate `gdx-proto/`.

**From the `gdx` meta-repo:** the umbrella also pins `gdx-proto` as a
submodule; cloning `gdx` with `--recurse-submodules` will fetch the same
proto tree into both `gdx-proto/` and any sibling SDK.

## WebSocket endpoints

The SDK builds its trading WebSocket URL by appending `/ws/v1` to the
configured base URL. Set the host via the `base_url` builder,
`GODARK_EDGE_URL`, or `GDX_EDGE_URL`; either `<host>` or `<host>/ws/v1`
resolve to the same endpoint.

| Environment | Canonical URL |
|---|---|
| Testnet (default) | `wss://api.godark-dex.com/ws/v1` |
| Localnet | `ws://127.0.0.1:4000/ws/v1` |

Public mainnet is not currently exposed; testnet is the live network for SDK
users today, and is the SDK default `base_url`. The public market-data client
(`MarketDataClient`) continues to target `<host>/ws/gomarket` regardless of
which suffix the caller supplied.

## Layout

- `src/`          — crate source
- `tests/`        — integration tests + mocks (live tests gated by `#[ignore]` + `GDX_LIVE_EDGE`)
- `examples/`    — runnable examples (`local_e2e.rs`, `full_trader_example.rs`)
- `build.rs`      — prost-build hook; reads .proto from `gdx-proto/proto/`
- `shared/`       — vendored symbol map (compiled into the crate via `include_str!`)
- `gdx-proto/`    — git submodule pinned to a specific commit on `v1/devnet` (see `.gitmodules`)

## Testing

Offline (default — live tests are `#[ignore]` and additionally short-circuit
when `GDX_LIVE_EDGE` is unset):

```bash
cargo test
```

Live tests target a real edge and are gated on `GDX_LIVE_EDGE=1`. Two
suites: WebSocket (`tests/ws_live_integration.rs`) and REST
(`tests/rest_live_integration.rs`). Both `#[ignore]`, so pass `--ignored`:

```bash
# Both suites against the SDK testnet default (api.godark-dex.com):
GDX_LIVE_EDGE=1 \
  GDX_API_KEY_ID=gdk_... GDX_API_SECRET=... GDX_PASSPHRASE=... \
  cargo test -p godark --test ws_live_integration --test rest_live_integration -- --ignored --nocapture

# REST only:
GDX_LIVE_EDGE=1 GDX_API_KEY_ID=... GDX_API_SECRET=... GDX_PASSPHRASE=... \
  cargo test -p godark --test rest_live_integration -- --ignored --nocapture

# WS only:
GDX_LIVE_EDGE=1 GDX_API_KEY_ID=... GDX_API_SECRET=... GDX_PASSPHRASE=... \
  cargo test -p godark --test ws_live_integration -- --ignored --nocapture
```

Environment variables (uniform across the Python / JS / C++ / Rust SDKs):

| Var | Default | Notes |
|---|---|---|
| `GDX_LIVE_EDGE` | `0` (skip) | Set to `1` to run live tests |
| `GDX_API_KEY_ID` + `GDX_API_SECRET` + `GDX_PASSPHRASE` | falls back to legacy `test-key-1` (override via `GDX_TEST_API_KEY`) | Production credentials |
| `GDX_REST_URL` / `GODARK_REST_URL` | `https://api.godark-dex.com` | REST live tests only |
| `GDX_EDGE_URL` / `GODARK_EDGE_URL` | `wss://api.godark-dex.com` | WS live tests only |
| `GDX_USE_DOCS_WIRE` | `1` (modern envelope) | Set `0|false|no|off` for legacy localnet edges (WS only) |
| `GDX_USER_UUID` / `GODARK_USER_UUID` | test fixture `00000000-0000-4000-8000-000000000001` | Optional client UUID override |
| `GDX_LIVE_SYMBOL` | `BTC-USDC-PERP` | REST live trading test symbol override |

Note: at the time of writing, the public testnet's sequencer is degraded;
live trading-flow tests fail with `POST /api/v1/session/setup HTTP 502`
(REST) or `ECDH session setup timed out` (WS). The
`*_bad_api_key_rejected` cases pass regardless and are the SDK-side
control confirming the failures are server-side.
