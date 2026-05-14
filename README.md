# GoDark Rust Examples (Darkpool MM Distribution)

This repository is a market-maker-facing distribution for GoDark's Rust SDK.
It includes:

- **pre-built example binaries** (`quickstart` + `full_trader_example`)
  shipped in a Linux x86_64 `.zip` release — no `cargo build` and no
  `protoc` required to run the examples
- the full **`godark` SDK source vendored under `sdk/`** for the local dev
  loop — no private crates registry required, no `protoc` required
  (pre-generated protobuf bindings ship with the SDK under
  `sdk/src/generated/`)
- a simple **`.env`** workflow (no shell `export` required)

Third-party crates (`tokio`, `prost`, `serde`, `reqwest`, …) are still
fetched from **crates.io** when you `cargo build` from source — only the
`godark` crate itself comes entirely from this repo.

## Prerequisites

| Item | Requirement |
|------|-------------|
| Rust | ≥ 1.79 (`rustup install stable`) (only needed for the dev loop / source build; not needed to run the released binaries) |
| OS | Linux x86_64 recommended (matches the published release zip) |

Released zips ship statically-linked binaries; no toolchain is required to
run them. From a git clone, install the toolchain once:

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

Optional:

- `GODARK_EDGE_URL` — local testing only; if unset, examples use `wss://api.godark-dex.com`.

The OS environment always wins over `.env`.

## Install

### From a released ZIP (recommended for MMs)

Download the latest `gdx-rust-sdk-examples-*-linux-x86_64.zip` from
[GitHub Releases](https://github.com/gq-godark/gdx-rust-sdk-examples/releases)
and unzip it. The bundle contains the prebuilt `quickstart` and
`full_trader_example` binaries plus `.env.example` at the bundle root.

```bash
unzip gdx-rust-sdk-examples-*-linux-x86_64.zip
cd gdx-rust-sdk-examples-*/
cp .env.example .env
# fill in GODARK_API_KEY_ID, GODARK_API_SECRET

./quickstart
./full_trader_example
```

The binaries are statically linked against the `godark` SDK at the pinned
upstream commit, so no Rust toolchain or `protoc` is required at the
recipient site.

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
| `quickstart` | `examples/quickstart.rs` | Minimal connect → LIMIT sell far from touch → cancel; demonstrates the symbolic `OrderError::error_code` reason on rejection |
| `full_trader_example` | `examples/full_trader_example.rs` | Reference bot flow with all 6 sequencer push callbacks (positions_snapshot, system_health, balance_update, margin_alert, funding_rate, settlement), order placement, modify, cancel, and queued-update drain |

Order-type support in this MM distribution is limited to **`MARKET`** and
**`LIMIT`**.

## Packaging for market makers

Build a release zip locally:

```bash
# Uses a sibling ../gdx-rust-sdk if present, else clones at the pinned ref:
./scripts/package.sh

# Or explicitly point at an upstream checkout:
UPSTREAM_SRC=/path/to/gdx-rust-sdk ./scripts/package.sh gdx-rust-sdk-vX.Y.Z-linux-x86_64
```

Output lands in the repo root as
`gdx-rust-sdk-examples-<bundle>-linux-x86_64.zip`. The zip includes:

- `quickstart` + `full_trader_example` — pre-built statically-linked binaries
- `README.md`, `SDK_REFERENCE.md` — recipient-facing docs from `bundle/`
- `.env.example` — credential template

Internal-only paths (`scripts/refresh_sdk.sh`, `scripts/package.sh`,
`.git/`, the vendored `sdk/`, build artifacts, local `.env`) are **not**
included in the zip.

**Release contract**: the release pipeline does *not* build from the
vendored `sdk/` tree. Every release build:

1. Reads the pinned upstream `gdx-rust-sdk` commit from `sdk/UPSTREAM_REF`.
2. Checks out `gq-godark/gdx-rust-sdk` at that exact ref into `./upstream/`.
3. Diffs the vendored `sdk/src` tree against `upstream/src` and **fails
   loudly** if they differ — this prevents hand-edits to `sdk/` from ever
   leaking into a release.
4. Builds the binaries by temporarily swapping the workspace's
   `godark = { path = "sdk" }` dependency to `godark = { path = "upstream" }`,
   so the recipient zip is byte-for-byte reproducible from a public commit
   hash.

The vendored `sdk/` therefore exists only for the local dev loop. The
source of truth for what ships is always `gdx-rust-sdk@<UPSTREAM_REF>`.

CI publishes a tagged
`gdx-rust-sdk-examples-*-linux-x86_64.zip` on every push to `main` via
`.github/workflows/release.yml`; download from
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
