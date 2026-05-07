# GoDark Rust examples

Sample programs that consume the **`godark`** crate from a **Cargo registry**
only. There is **no** Git or source dependency on any SDK source tree:
configure a Cargo registry (e.g. [crates.io](https://crates.io) or a private
registry such as [Cloudsmith](https://cloudsmith.io) / Artifactory) that
publishes a matching `godark` version and run `cargo build --examples`.

This repo is the Rust counterpart of [`gdx-cpp-sdk-examples`](https://github.com/gq-godark/gdx-cpp-sdk-examples)
and ships the same set of binaries.

## Prerequisites

- Rust ≥ 1.79 (`rustup install stable`)
- Cargo (bundled with the toolchain)
- Network access to your Cargo registry (or a `[patch.crates-io]` block — see
  the comment in `Cargo.toml`)

## Build

From the **repository root**:

```bash
cargo build --examples --release
```

To run a specific example:

```bash
cargo run --example quickstart --release
```

## Binaries (all crates.io / public API)

| Target | Source | What it does |
|--------|--------|--------------|
| `quickstart` | `examples/quickstart.rs` | Minimal connect → limit buy → cancel. Needs `GODARK_API_KEY_ID` + `GODARK_API_SECRET` (or pass them as CLI args). |
| `e2e_trading_smoke` | `examples/e2e_trading_smoke.rs` | Scripted E2E check with `--auth-only`; exit codes for CI. |
| `market_data_example` | `examples/market_data_example.rs` | Public gomarket order book + trades (no keys). |
| `full_trader_example` | `examples/full_trader_example.rs` | Larger demo: callbacks, MD client, place/modify/cancel, queued-update drain. |
| `full_trader_rest` | `examples/full_trader_rest.rs` | REST-only `GodarkRestClient`: session + encrypted place + cancel (`GDX_REST_URL`, keys). |

### Configure (`.env`)

Each binary calls a small `dotenvy::dotenv()` helper on startup so a
sibling `.env` file is honored automatically. Copy the template and fill
in the values you need:

```bash
cp .env.example .env
$EDITOR .env       # set GODARK_API_KEY_ID, GODARK_API_SECRET, GODARK_EDGE_URL...
```

OS environment variables always win over `.env` (matching python-dotenv,
the JS `dotenv` package, and the C++ examples), so `KEY=value cargo run`
still overrides whatever is in the file.

The same `.env` configures every example:

- **Credentials:** `GODARK_API_KEY_ID` + `GODARK_API_SECRET`, or a single
  bare `GODARK_API_KEY` (localnet `test-key-1`). `GDX_*` aliases are
  honored as a fallback.
- **WebSocket endpoint:** `GODARK_EDGE_URL` (e.g. `ws://localhost:4000`
  for localnet; defaults to `wss://api.godark-dex.com`).
- **REST endpoint:** auto-derived from `GODARK_EDGE_URL`
  (`ws[s]://...` → `http[s]://...`); override with `GODARK_REST_URL`
  when REST and WebSocket live on different hosts.
- **Resting price:** `GODARK_TEST_LIMIT_PRICE` for environments that cap
  mark deviation (localnet caps at 1000 bps).
- **TLS dev bypass:** `GODARK_TLS_SKIP_VERIFY=1` (or `GDX_*`).

## Layout

| Path | Purpose |
|------|---------|
| `Cargo.toml` | Requires `godark = "0.1"` from a Cargo registry; declares each example as a `[[example]]` target |
| `examples/*.rs` | Sources for each binary, identical to the SDK's own in-tree examples |
| `.gitignore` | Excludes `target/` and other build artefacts |

## `Cargo.toml`

Edit the `godark` version line in `Cargo.toml` to match what your Cargo
registry provides. Local SDK developers can swap the registry dep for a
`[patch.crates-io]` path entry — see the comment block in `Cargo.toml`.
