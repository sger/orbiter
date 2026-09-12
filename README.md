# Orbiter

A company IPA inspection workspace for macOS and Windows, built with Rust, Tauri 2, React, and TypeScript. **Phase 1 is implemented. Signing, authentication, device discovery, installation, and automatic refresh are not implemented.** Those controls are explicitly unavailable.

## Run

Install stable Rust, Node.js 22.12+ (Node 24 LTS preferred), and the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/). Development here was verified with Rust 1.98.0, Node 23.5.0, and npm 10.9.2 on Apple Silicon macOS.

```sh
npm ci
npm run tauri dev
```

Choose an IPA or drop one file into the desktop window. The UI reads real metadata through the Rust core. `npm run dev` runs a browser preview; native operations are disabled there. No sample devices, accounts, or successful signing results are provided.

Run inspection without the desktop shell:

```sh
cargo run --locked -p orbiter-core -- /path/to/company.ipa
```

The CLI writes a JSON inspection report to stdout. It omits full device allowlists and profile certificate data, but includes app identifiers and capabilities; handle reports as company information. The CLI takes a file path only, never credentials. Local reference files are not fixtures or runtime dependencies.

## Build and verify

```sh
npm run build
cargo test --locked -p orbiter-core
cargo clippy --locked --workspace --all-targets -- -D warnings
npm test
npm run tauri build -- --debug --bundles app  # macOS local development bundle
npm run tauri build                         # native release packaging
```

Browser tests require installed Google Chrome; change `channel` in `playwright.config.ts` to use a provisioned Playwright browser. They use synthetic IPC responses and do **not** validate WKWebView, Windows WebView2, or physical-device behavior. The built macOS debug app is `target/debug/bundle/macos/Orbiter.app`. It is a local development artifact, not a notarized release.

For Windows builds, use a Windows host with MSVC C++ build tools, Rust's MSVC toolchain, and WebView2 as described by Tauri. No Windows build or device test has been performed here. Phase 1 does not need Apple device drivers. Later USB work requires validating Apple Mobile Device services/USB drivers; the evaluated iloader project currently directs Windows users to iTunes. This repository does not download or install drivers.

## What works

- Read-only ZIP inspection with resource limits, path validation, duplicate/case-collision detection, and rejection of symlinks/special files.
- XML and binary Info.plists, main app, nested apps, Watch apps, extensions, and framework inventory.
- Thin/fat Mach-O architecture, encryption flags, and embedded XML entitlements; DER presence is detected.
- CMS profile payload decoding, original signing team, expiration, distribution inference, device **counts**, and profile authorizations. No CMS signature/trust claim.
- PNG icons when a standard PNG is declared in bundle metadata; fallback for asset catalogs and Apple CgBI PNGs.
- Contextual compatibility findings, detailed per-bundle inspection, stage progress, cooperative cancellation, and actionable errors.

The original IPA is never written, extracted, or executed. Inspection makes no network requests. No passwords, tokens, signing keys, or pairing records are requested or stored. Desktop logs contain operation/stage and success fields only, not filenames, report payloads, or device identifiers.

See [architecture and limits](docs/architecture.md), [dependency decisions and Apple requirements](docs/decisions.md), and the [validation matrix and next milestones](docs/validation.md).
