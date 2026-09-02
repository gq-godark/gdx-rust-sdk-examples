# GoDark Rust Examples (Darkpool MM Distribution)

This repository is a market-maker-facing distribution for GoDark's Rust SDK.
It includes:

- the full **`godark` SDK source vendored under `sdk/`** for the local dev
  loop — no private crates registry required, no `protoc` required
  (pre-generated protobuf bindings ship with the SDK under
  `sdk/src/generated/`)
- **example sources** (`quickstart` + `full_trader_example`) shipped in a
  `.zip` release — recipients build with `cargo build`
- a simple **`.env`** workflow (no shell `export` required)

Third-party crates (`tokio`, `prost`, `serde`, `reqwest`, …) are still
fetched from **crates.io** when you `cargo build` from source — only the
`godark` crate itself comes entirely from this repo.

## Prerequisites

| Item | Requirement |
|------|-------------|
| Rust | ≥ 1.79 (`rustup install stable`) |
| OS | any platform Rust supports (Linux, macOS, Windows; amd64, arm64, …) |

Install the toolchain once:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
```

## Testnet onboarding

Before running the examples, complete this setup flow:

1. Open the testnet frontend: `https://app.godark-dex.com`
2. Create an account using email sign-up.
3. Fund your testnet account using the faucet: `https://faucet.godark-dex.com`
4. In the frontend, go to **Settings → API Key Management** and click **Create API Key**.
5. Use the generated key ID and secret for your local `.env`.

## Configure credentials

Copy `.env.example` to `.env` and fill in your API credentials:

```bash
cp .env.example .env
```

Required keys:

- `GODARK_API_KEY_ID`
- `GODARK_API_SECRET`
- `GODARK_PASSPHRASE` — required for API key-pair auth. If it contains `$`,
  wrap it in single quotes (`GODARK_PASSPHRASE='...'`) — `dotenvy` expands
  unquoted `$VAR` sequences and will silently truncate the value.

Optional:

- `GODARK_EDGE_URL` — override the edge URL (default: public testnet `wss://api.godark-dex.com` via the SDK Testnet environment preset).
- `GDX_HPKE_STATIC_PUBLIC_KEY` — sequencer HPKE static public key (64 hex). Required for **localnet/devnet** encrypted trading Aliases: `GDX_HPKE_STATIC_PUBKEY`, `GODARK_HPKE_STATIC_PUBLIC_KEY`, `VITE_GDX_HPKE_STATIC_PUBKEY`.

The OS environment always wins over `.env`.

## Localnet (`gdx up`)

```bash
GODARK_EDGE_URL=ws://127.0.0.1:13300
GODARK_API_KEY=test-key-1
GDX_HPKE_STATIC_PUBLIC_KEY=1d61f116451fdfda1aa4aaf50b7200c3b362d0445bfa2d7ef1f80b3b8881a533
gdx fund 00000000-0000-4000-8000-000000000001
```

Copy `VITE_GDX_HPKE_STATIC_PUBKEY` from `gdx-web/.env.localnet` if your pin differs.

## Install

### From a released ZIP (recommended for MMs)

Download the latest `godark-rust-sdk-*.zip` from
[GitHub Releases](https://github.com/gq-godark/gdx-rust-sdk-examples/releases)
and unzip it. The bundle contains the example sources under `examples/`, the
vendored `godark` crate under `sdk/`, and a top-level `Cargo.toml` so
recipients build with `cargo build --release --examples`.

```bash
unzip godark-rust-sdk-*.zip
cd godark-rust-sdk-*/
cp .env.example .env
# fill in GODARK_API_KEY_ID, GODARK_API_SECRET, GODARK_PASSPHRASE

cargo build --release --examples
./target/release/examples/quickstart
./target/release/examples/full_trader_example
```

The bundled `Cargo.toml` wires `godark = { path = "sdk" }`, so `cargo`
only fetches the third-party runtime crates from `crates.io`; the SDK
itself comes from the vendored copy in the zip.

### From a git clone (development)

```bash
git clone https://github.com/gq-godark/gdx-rust-sdk-examples.git
cd gdx-rust-sdk-examples
cp .env.example .env
# fill in credentials

cargo build --release --examples
cargo run   --release --example quickstart
cargo run   --release --example full_trader_example
```

Built binaries land in `target/release/examples/`. The local dev loop uses
the vendored `sdk/` (`godark = { path = "sdk" }` in `Cargo.toml`) so you
get fast incremental builds and IDE go-to-definition into the SDK source.

## Examples

| Sample | Source | Purpose |
|--------|--------|---------|
| `quickstart` | `examples/quickstart.rs` | Minimal connect → `subscribe(["orders"])` → LIMIT sell far from touch → cancel (book confirmation needs the private orders channel) |
| `full_trader_example` | `examples/full_trader_example.rs` | Reference bot flow with all 6 sequencer push callbacks, place / modify / cancel, mass-quote / batch-cancel, and queued-update drain |

Order-type support in this MM distribution is limited to **`MARKET`** and
**`LIMIT`**.

## Packaging for market makers

Build a release zip locally:

```bash
# Uses a sibling ../gdx-rust-sdk if present, else clones at the pinned ref:
./scripts/package.sh

# Or explicitly point at an upstream checkout:
UPSTREAM_SRC=/path/to/gdx-rust-sdk ./scripts/package.sh godark-rust-sdk-vX.Y.Z
```

Output lands in the repo root as
`godark-rust-sdk-<bundle>.zip`. The zip includes:

- `Cargo.toml` — workspace manifest (`godark = { path = "sdk" }`) for source builds
- `examples/*.rs` — example source files (`quickstart.rs`, `full_trader_example.rs`, `dotenv.rs`)
- `sdk/` — bundled `godark` crate source
- `README.md`, `SDK_REFERENCE.md` — recipient-facing docs from `bundle/`
- `.env.example` — credential template

Maintainer-only paths (`scripts/`, `bundle/`, `target/`, `.git/`, local
`.env`) are **not** included in the zip.

The CI release pipeline additionally runs a recipient smoke step that
unzips the bundle and `cargo build --release --examples` against the
included sources, confirming the bundle is build-complete on its own.

**Release contract**: hand-edits to the vendored `sdk/` tree must never
leak into a release. Every release build:

1. Reads the pinned upstream `gdx-rust-sdk` commit from `sdk/UPSTREAM_REF`.
2. Checks out `gq-godark/gdx-rust-sdk` at that exact ref into `./upstream/`.
3. Parity check — diffs the vendored `sdk/src` tree against `upstream/src`
   and **fails loudly** if they differ. Once this passes, the vendored
   `sdk/` is byte-equal to upstream for every file the workspace compiles,
   so building against `sdk/` is equivalent to building against `upstream/`.
4. Smoke-builds the examples via `cargo build --release --examples` against
   the parity-verified `sdk/`.
5. Stages the parity-verified `sdk/`, the example sources, the workspace
   `Cargo.toml`, and recipient docs into the zip — recipients build with
   `cargo build --release --examples` from the unzipped bundle.

The source of truth for what ships is always
`gdx-rust-sdk@<sdk/UPSTREAM_REF>`.

CI publishes a tagged `godark-rust-sdk-*.zip` on every push to
`main` via `.github/workflows/release.yml`; download from
[GitHub Releases](https://github.com/gq-godark/gdx-rust-sdk-examples/releases).

## Layout

| Path | Purpose |
|------|---------|
| `examples/` | Source for runnable MM examples (`quickstart.rs`, `full_trader_example.rs`, `dotenv.rs` helper) |
| `Cargo.toml` | Examples crate; depends on the vendored `godark` via `path = "sdk"` |
| `sdk/` | Vendored `godark` SDK source (with pre-generated protobuf bindings under `sdk/src/generated/`) |
| `sdk/UPSTREAM_REF` | Pinned upstream `gdx-rust-sdk` commit; CI rebuilds against this exact ref |
| `bundle/README.md` | Recipient-facing README packaged into the release zip |
| `bundle/SDK_REFERENCE.md` | Recipient-facing API reference packaged into the release zip |
| `SDK_REFERENCE.md` | Maintainer-grade API reference; mirrored in trimmed form at `bundle/SDK_REFERENCE.md` |
| `.env.example` | Credential template copied to `.env` |
| `scripts/refresh_sdk.sh` | Refresh `sdk/` from a sibling SDK checkout + write `sdk/UPSTREAM_REF` (maintainers only; not shipped) |
| `scripts/package.sh` | Produce the release zip (CI + local) |
| `.github/workflows/release.yml` | Build / smoke / publish the release zip on every push and PR |
| `.github/workflows/auto-bump-sdk-pin.yml` | Layer 2 listener that auto-PRs vendored SDK refreshes when upstream `gdx-rust-sdk` ships |

## Refreshing `sdk/` (internal)

From a sibling development checkout of the upstream SDK at the commit you
want to ship:

```bash
./scripts/refresh_sdk.sh /path/to/gdx-rust-sdk
git add sdk/
git commit -m "refresh: sync vendored sdk with upstream"
```

The script refuses to run if the sibling SDK checkout is dirty,
`rsync`'s `src/` and the trimmed `Cargo.toml` into `sdk/`, and writes the
upstream HEAD commit (or tag, if HEAD is on one) to `sdk/UPSTREAM_REF`.

The Layer 2 listener (`auto-bump-sdk-pin.yml`) wraps this loop into a
rolling auto-PR triggered by `gdx-rust-sdk` pushes to `main`. The full
upstream-change chain (proto → SDK → examples → release zip) is:

1. A push to `gdx-proto` (`v1/devnet`) dispatches `gdx-proto-changed` to
   `gdx-rust-sdk`.
2. `gdx-rust-sdk/.github/workflows/auto-regen-protos.yml` regenerates the
   committed proto bindings and opens a rolling PR. Merging it dispatches
   `gdx-sdk-changed` to this repo.
3. `auto-bump-sdk-pin.yml` here refreshes `sdk/`, bumps `sdk/UPSTREAM_REF`,
   and opens its own rolling PR.
4. Merging that PR triggers `release.yml`, which rebuilds the bundle zip
   from the new pin and publishes a tagged GitHub Release.
