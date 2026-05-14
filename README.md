# GoDark Rust Examples (Darkpool MM Distribution)

Maintainer-facing repository for the GoDark Rust SDK examples. The release
artifact (a Linux x86_64 `.zip` containing pre-built example binaries +
recipient docs from `bundle/`) is produced by the CI release pipeline on
every push to `main`. For the recipient-facing view of what ships in that
zip, see [`bundle/README.md`](bundle/README.md) and
[`bundle/SDK_REFERENCE.md`](bundle/SDK_REFERENCE.md).

This repository includes:

- two minimal darkpool trading examples (`quickstart` + `full_trader_example`)
- the full `godark` SDK source vendored under `sdk/` for local dev — no
  private registry required, no `protoc` required (pre-generated protobuf
  bindings ship with the SDK under `sdk/src/generated/`)
- a simple `.env` workflow (no shell `export` required)
- a `scripts/refresh_sdk.sh` that resyncs `sdk/` from a sibling
  `gdx-rust-sdk` checkout and pins the upstream commit in `sdk/UPSTREAM_REF`
- a `scripts/package.sh` that produces the Linux x86_64 `.zip` shipped to
  MMs, built strictly from the pinned upstream `gdx-rust-sdk` commit (not
  from the vendored copy — see *Release contract* below)
- a `.github/workflows/release.yml` that runs `scripts/package.sh` on every
  push and pull request, and publishes a tagged GitHub Release on every
  merge to `main`

## Dev loop (build from source)

### Prerequisites

- Rust ≥ 1.79 (`rustup install stable`)
- Cargo (bundled with the toolchain)
- Network access to `crates.io` for the standard runtime crates the SDK
  pulls in (`tokio`, `prost`, `serde`, `reqwest`, etc.). The `godark` SDK
  itself is vendored under `sdk/`; you do not need access to any private
  registry.

### Build + run

```bash
cargo build --release --examples
cargo run   --release --example quickstart
cargo run   --release --example full_trader_example
```

Built binaries land in `target/release/examples/`.

### Configure credentials

```bash
cp .env.example .env
$EDITOR .env       # set GODARK_API_KEY_ID, GODARK_API_SECRET
```

Required:

- `GODARK_API_KEY_ID`
- `GODARK_API_SECRET`

Optional:

- `GODARK_EDGE_URL` — defaults to `wss://api.godark-dex.com`.

The OS environment always wins over `.env`. See *Testnet onboarding* in
[`bundle/README.md`](bundle/README.md) for the API-key minting flow.

## Examples

| Target | Source | Purpose |
|--------|--------|---------|
| `quickstart` | `examples/quickstart.rs` | Minimal connect → place limit sell → cancel; demonstrates the symbolic `OrderError::error_code` reason on rejection. |
| `full_trader_example` | `examples/full_trader_example.rs` | Full darkpool trading flow with all 6 sequencer push callbacks (`positions_snapshot`, `system_health`, `balance_update`, `margin_alert`, `funding_rate`, `settlement`), order placement, modify, cancel, and queued-update drain. |

Order-type support in this MM distribution is limited to `MARKET` and `LIMIT`.

## Release contract

The release pipeline does **not** build from the vendored `sdk/` tree.
Instead, every release build:

1. Reads the pinned upstream `gdx-rust-sdk` commit from `sdk/UPSTREAM_REF`.
2. Checks out `gq-godark/gdx-rust-sdk` at that exact ref into `./upstream/`.
3. Diffs the vendored `sdk/src` tree against `upstream/src` and **fails
   loudly** if they differ — this prevents hand-edits to `sdk/` from ever
   leaking into a release.
4. Builds the binaries by temporarily swapping the workspace's
   `godark = { path = "sdk" }` dependency to `godark = { path = "upstream" }`,
   so the recipient zip is byte-for-byte reproducible from a public commit
   hash.
5. Stages the binaries, `bundle/README.md`, `bundle/SDK_REFERENCE.md`, and
   `.env.example` into a `gdx-rust-sdk-examples-<version>-linux-x86_64.zip`.

The vendored `sdk/` therefore exists only for the local dev loop (faster
`cargo build`, IDE go-to-definition, etc.). The source of truth for what
ships is always `gdx-rust-sdk@<UPSTREAM_REF>`.

## Refreshing `sdk/` from a sibling SDK checkout (maintainer)

```bash
./scripts/refresh_sdk.sh /path/to/gdx-rust-sdk
```

The script:

- refuses to run if the sibling SDK checkout is dirty (uncommitted changes)
- rsyncs `src/` and the trimmed `Cargo.toml` into `sdk/`
- writes the upstream HEAD commit (or tag, if HEAD is on one) to
  `sdk/UPSTREAM_REF`

Commit the resulting diff under `sdk/` together with the bumped
`sdk/UPSTREAM_REF` so CI's parity check stays green.

## Building the release zip locally (maintainer)

The same script CI runs:

```bash
# Uses a sibling ../gdx-rust-sdk if present, else clones at the pinned ref:
./scripts/package.sh

# Or explicitly point at an upstream checkout:
UPSTREAM_SRC=/path/to/gdx-rust-sdk ./scripts/package.sh gdx-rust-sdk-vX.Y.Z-linux-x86_64
```

Output lands in the repo root as
`gdx-rust-sdk-examples-<bundle>-linux-x86_64.zip`.

## CI / release pipeline

Workflows under `.github/workflows/`:

| Workflow | Trigger | What it does |
|----------|---------|--------------|
| `release.yml` | push + PR to `main` | Build the release zip from the pinned upstream commit, file/ldd-smoke the binaries; on push to `main`, additionally tag and publish a GitHub Release with the zip attached. |
| `auto-bump-sdk-pin.yml` | `repository_dispatch` (from `gdx-rust-sdk/main`) or manual | Refresh `sdk/` from the dispatched upstream SHA, bump `sdk/UPSTREAM_REF`, force-push to `auto/bump-sdk-pin` and open a rolling PR if any drift. |

The full upstream-change chain (proto → SDK → examples → release zip):

1. A push to `gdx-proto` (`v1/devnet`) dispatches `gdx-proto-changed` to `gdx-rust-sdk`.
2. `gdx-rust-sdk/.github/workflows/auto-regen-protos.yml` regenerates the
   committed proto bindings and opens a rolling PR. Merging it dispatches
   `gdx-sdk-changed` to this repo.
3. `auto-bump-sdk-pin.yml` here refreshes `sdk/`, bumps `sdk/UPSTREAM_REF`,
   and opens its own rolling PR.
4. Merging that PR triggers `release.yml`, which rebuilds the bundle zip
   from the new pin and publishes a tagged GitHub Release.

## Required repository secrets

| Secret | Used by | Purpose |
|--------|---------|---------|
| `GDX_APP_ID` + `GDX_APP_PRIVATE_KEY` | `release.yml` + `auto-bump-sdk-pin.yml` | Credentials for the `godark-ci` GitHub App. The app must be installed on `gq-godark` with `contents:read` on `gdx-rust-sdk` (for `release.yml`'s pinned upstream checkout) and `contents:write` on `gdx-rust-sdk-examples` (for the listener's rolling-PR push). Single secret pair powers both workflows. |

## Layout

| Path | Purpose |
|------|---------|
| `examples/quickstart.rs` | Minimal connect / place / cancel example |
| `examples/full_trader_example.rs` | Reference bot flow with all 6 push callbacks |
| `examples/dotenv.rs` | Tiny shared helper (`load_dotenv` + symbolic error printer) |
| `Cargo.toml` | Examples crate; depends on the vendored `godark` via `path = "sdk"` |
| `sdk/` | Vendored `godark` SDK source (with pre-generated protobuf bindings under `sdk/src/generated/`) |
| `sdk/UPSTREAM_REF` | Pinned upstream `gdx-rust-sdk` commit; CI rebuilds against this exact ref |
| `.env.example` | Credential template for local `.env` |
| `README.md` | This file (maintainer guide) |
| `SDK_REFERENCE.md` | Maintainer-grade API reference; mirrored in trimmed form at `bundle/SDK_REFERENCE.md` |
| `bundle/README.md` | Recipient-facing README packaged into the release zip |
| `bundle/SDK_REFERENCE.md` | Recipient-facing API reference packaged into the release zip |
| `scripts/refresh_sdk.sh` | Refresh `sdk/` from a sibling SDK checkout + write `sdk/UPSTREAM_REF` |
| `scripts/package.sh` | Produce the release zip (CI + local) |
| `.github/workflows/release.yml` | Build / smoke / publish the release zip |
| `.github/workflows/auto-bump-sdk-pin.yml` | Refresh `sdk/` automatically on `gdx-rust-sdk` push events |
