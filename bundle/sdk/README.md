# GoDark Rust SDK

Encrypted Rust client for the GoDark DEX over WebSocket.

## WebSocket endpoints

The SDK appends `/ws/v1` to the configured base URL. Set the host via the
`base_url` builder or `GODARK_EDGE_URL` (either `<host>` or `<host>/ws/v1`
resolve to the same endpoint).

| Environment | Canonical URL |
|---|---|
| Testnet (default) | `wss://api.godark-dex.com/ws/v1` |
| Localnet | `ws://127.0.0.1:4000/ws/v1` |

## Layout

- `src/` — crate source
- `shared/symbols.json` — symbol map snapshot
