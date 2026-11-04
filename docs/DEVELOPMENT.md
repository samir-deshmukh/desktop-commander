# Development Setup (Linux Mint)

This guide records the environment and commands verified for Sam's Desktop Commander. It is written for the Linux Mint 22.x development machine used by this repository.

## Verified environment

The current development machine reports:

- Linux Mint 22.3
- Rust 1.99.1 and Cargo 1.99.1
- Node.js 22.23.3 and npm 10.9.9
- GTK 3.24.41
- WebKitGTK 4.1 / WebKit 2.52.6
- librsvg 2.58.0
- Ayatana Application Indicator development package installed

The Rust package declares `rust-version = "1.90"`. Use a Rust toolchain at or above that version. The exact Node/npm versions above are the versions used for the successful frontend checks.

## Linux system dependencies

Install the Tauri Linux build dependencies:

```bash
sudo apt update
sudo apt install -y \
  build-essential \
  curl \
  wget \
  file \
  pkg-config \
  libssl-dev \
  libgtk-3-dev \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev
```

If Rust is not installed, install it through the official rustup distribution and ensure `cargo` and `rustc` are on `PATH`. Install Node.js 22 LTS and npm.

## Get the source

```bash
git clone https://github.com/samir-deshmukh/sams-desktop-commander.git
cd sams-desktop-commander
```

The Rust lockfile is `app/src-tauri/Cargo.lock`; the frontend lockfile is `app/package-lock.json`. Keep both lockfiles committed and use locked/reproducible install commands.

## Install frontend dependencies

```bash
cd app
npm ci
```

## Run the desktop application in development

From the `app` directory:

```bash
npx tauri dev -- --bin app
```

The explicit `--bin app` selects the Tauri desktop binary because the Rust package also defines the separate `sam-desktop-mcp` binary.

## Verify the project

Use one Cargo build job on low-memory machines to reduce peak memory use.

```bash
cd app/src-tauri
cargo fmt --check
CARGO_BUILD_JOBS=1 cargo test --lib --features desktop --locked
CARGO_BUILD_JOBS=1 cargo test --no-default-features --features mcp --locked
CARGO_BUILD_JOBS=1 cargo check --features desktop --locked
```

Then verify the frontend:

```bash
cd ../
npm run lint
npm run build
```

The frontend build may report a Vite chunk-size advisory for the main JavaScript bundle. It is a warning, not a build failure.

## Build the standalone MCP server

```bash
cd app/src-tauri
CARGO_BUILD_JOBS=1 cargo build --no-default-features --features mcp --bin sam-desktop-mcp --locked
```

The development binary is at `app/src-tauri/target/debug/sam-desktop-mcp`.

## Low-memory development notes

- Keep `CARGO_BUILD_JOBS=1` for Rust builds on machines with limited RAM.
- Avoid running a desktop build, test build, and MCP build simultaneously.
- Prefer `npm ci` over `npm install` when restoring dependencies from the committed lockfile.
- Build artifacts are local and ignored by Git; do not commit `target/`, `node_modules/`, or `dist/`.

## Clean-checkout verification status

The repository is linked to the public GitHub remote and both dependency lockfiles are tracked. The build, test, lint, and frontend production-build commands above have passed on the development checkout. A second full Rust compilation from a fresh checkout has not been run because it would duplicate build work and increase memory/disk pressure on the low-resource development machine.
