# Orbiter

A company IPA desktop workspace for macOS and Windows, built with Rust, Tauri 2, React, and TypeScript.

It exists for one problem: a company team's 100-device allowance is full, so testers outside it cannot install the company build. Orbiter re-signs that build with a tester's own free Apple ID and installs it on their iPhone.

The existing signing/install path has previously been run end to end on a physical device: Apple sign-in, device registration, a development certificate whose key stays in this Mac's Keychain, App ID and profile registration, re-signing, installation, and the app launching. **Re-signing on a schedule is not implemented, by design** — a free team's profile expires after seven days, and Orbiter tracks when an installed build runs out and says so, but re-signing is always a click. Nothing here contacts Apple on its own. Federated company accounts are not supported. Windows is unverified.

## Run

Install stable Rust, Node.js 22.12+ (Node 24 LTS preferred), and the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/). Development here was verified with Rust 1.98.0, Node 23.5.0, and npm 10.9.2 on Apple Silicon macOS.

```sh
npm ci
npm run tauri dev
```

Use **Import IPA** in the IPAs library to save one or more local copies. Open an app to browse versions, installation history, and remembered devices, then open a version to sign or review installation. The UI reads real metadata through the Rust core. `npm run dev` runs a browser preview; native operations are disabled there. Only real connected-device data is shown; no sample accounts or fabricated results are provided.

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

For Windows builds, use a Windows host with MSVC C++ build tools, Rust's MSVC toolchain, and WebView2 as described by Tauri. No Windows build or device test has been performed here. Phase 1 does not need Apple device drivers. USB discovery on Windows requires validating Apple Mobile Device services/USB drivers; the evaluated iloader project currently directs Windows users to iTunes. This repository does not download or install drivers.

## What works

- Read-only ZIP inspection with resource limits, path validation, duplicate/case-collision detection, and rejection of symlinks/special files.
- XML and binary Info.plists, main app, nested apps, Watch apps, extensions, and framework inventory.
- Thin/fat Mach-O architecture, encryption flags, and embedded XML entitlements; DER presence is detected.
- CMS profile payload decoding, original signing team, expiration, distribution inference, device **counts**, and profile authorizations. No CMS signature/trust claim.
- PNG icons when a standard PNG is declared in bundle metadata; fallback for asset catalogs and Apple CgBI PNGs.
- Contextual compatibility findings, detailed per-bundle inspection, stage progress, cooperative cancellation, and actionable errors.

The original IPA is never written, extracted, or executed. Inspection makes no network requests. Account credentials are requested only in the explicit desktop sign-in flow described below; passwords and account sessions are not persisted. Discovery reads existing pairing records from the local Apple device service into memory to verify a session; records are never printed, exported, or persisted by Orbiter. Desktop logs contain operation/stage and success fields only, not filenames, report payloads, or device identifiers.

See [architecture and limits](docs/architecture.md), [dependency decisions and Apple requirements](docs/decisions.md), and the [validation matrix and next milestones](docs/validation.md).

## Connected iPhone discovery

Connect and unlock an iPhone. The device selector refreshes automatically; **Refresh** forces a new check. It shows name/model, USB or network transport, iOS version, and verified pairing or a recovery message. A verified pairing session does not prove the device is unlocked, Developer Mode is enabled, or an IPA is authorized for installation.

```sh
cargo run --locked -p orbiter-core --bin orbiter-devices
```

This CLI returns device display metadata and ephemeral transport IDs, not UDIDs or pairing secrets. If trust is missing, establish it in Finder or Apple's Windows device app and approve it on the phone. Orbiter does not initiate pairing. Discovery uses only `/var/run/usbmuxd` on macOS or loopback port 27015 on Windows, ignores remote-daemon environment overrides, and performs no account/portal requests.

## Install an already-signed IPA

1. Select the company IPA and a paired USB iPhone.
2. In **Install existing signature**, choose **Review installation**. This makes a private local snapshot, checks iPhone platform/OS support, executable inspection, profile expiry and the phone's profile membership, and queries only the selected bundle ID on the phone. It does not upload or install the IPA.
3. Review the app/device, existing-app warning, blockers, notes, and snapshot fingerprint. Reviews expire after ten minutes and are invalidated when the selected IPA/device changes.
4. If there are no blockers, acknowledge the replacement/data-retention consequences and choose **Install unchanged IPA**. This uploads the snapshot over AFC, then asks iOS to install it. No signing identity is changed and no Watch bundle or entitlement is stripped.
5. Keep the phone connected. **Cancel transfer** works before dispatching installation; after dispatch, iOS owns the operation and cancellation is unavailable. A lost connection after dispatch is an **unknown outcome**, never automatic success or an automatic retry.

An already-installed app with the same bundle ID may be replaced. iOS still validates signatures/provisioning; the static review is not cryptographic verification or a guarantee of installability. Watch-device authorization and runtime features require separate tests. Orbiter neither uninstalls apps nor changes Apple portal state.

Read-only review from the CLI (there is deliberately no installation CLI command):

```sh
cargo run --locked -p orbiter-core --bin orbiter-install-review -- /path/to/company.ipa 1
```

Replace `1` with the ephemeral transport ID from `orbiter-devices`. The CLI discards its snapshot on exit. Review output includes app/device display information; handle it as private company information.

The desktop keeps only the latest job's stage and redacted status in `last-install.json` under its app-data directory, and what an installed build's expiry line needs in `renewal.json` beside it — app name, identifier, expiry, and a tag derived from the team, never the team identifier, the IPA's location, the phone, or the Apple ID. Both files are bounded and refused when damaged rather than trusted. After an interrupted install, check the phone before making a new review. There is no automatic retry, resumable upload, or background refresh. Normal completion/cancellation attempts to remove only its UUID-named staging IPA. Disconnections or process termination may leave that staging file or a local temporary snapshot behind; automatic orphan cleanup is not implemented. Run one Orbiter instance at a time.

## Re-sign for a tester's Apple ID (macOS)

1. Run `npm run tauri dev`. A connected iPhone or IPA is not required.
2. Confirm direct Apple authentication and enter the tester's account and password in the application only. Local macOS authentication support is resolved as part of signing in; its failure stops sign-in and says so.
3. Choose **Sign in to Apple**, complete trusted-device/SMS verification, and select the signing team explicitly. Nothing is auto-selected.
4. Work through the four steps: register the iPhone on the team, get a development certificate, register the plan's rewritten identifiers and download their profiles, then **Re-sign IPA**.
5. **Review installation** and install the re-signed build. On the iPhone, trust the developer under Settings → General → VPN & Device Management before launching; this is asked once per certificate.
6. **Refresh teams** checks developer access again. **Sign out** clears the local session. Sessions expire on access after 30 minutes or when the process exits.

The re-signed build installs under a rewritten bundle identifier, so it appears as a **second app** beside the company one rather than replacing it. Apple grants a free personal team no capabilities, so push notifications, universal links, Apple Pay and app-group sharing do not work in it, and any service that recognises the app by its bundle identifier — social sign-in, a backend that pins it — will not recognise the re-signed one until that identifier is registered with it.

No remote Anisette provider or proxy fallback is available. Local support failure stops sign-in. Passwords/codes are never persisted; account sessions remain in memory. macOS manages its own authentication support data. Windows local authentication is not yet implemented. Direct Apple HTTPS access is required, including on corporate networks.

Every step that writes to the Apple account — registering a device, requesting a certificate, registering identifiers — is behind its own explicit acknowledgement, and none happens automatically. See [local authentication and data handling](docs/local-authentication.md), including cleanup of any obsolete state from the retired remote preview.

## Device log capture (macOS)

After signing, **Device log → Capture while you reproduce it** streams the connected iPhone's system log and keeps only the lines mentioning the signed build's identifier or app name; every other line the device emits is counted and discarded. Nothing is written to disk, and the capture stops on request, after five minutes, or after 500 matching lines. Use it to see what iOS actually reports when a screen in the re-signed app fails — a denied entitlement, or a service refusing the rewritten bundle identifier — instead of inferring it.

## Persistent app library

The library retains originals and successfully signed outputs in local application storage. History belongs to the reviewed artifact and verified device; it is not a complete inventory of a phone. Removing saved files never uninstalls apps. See [library architecture and verification](docs/app-library.md) for storage, recovery, privacy, and removal behavior. The new import → sign → install → restart → reopen library flow has not yet been validated on a physical device.
