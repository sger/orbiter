# Orbiter

A company IPA desktop workspace for macOS and Windows, built with Rust, Tauri 2, React, and TypeScript. Inspection, device discovery, reviewed existing-signature installation, and a **Phase 3 Apple account sign-in preview** are implemented. The user confirmed installation and app operation on an already provisioned iPhone. Re-signing, provisioning changes, and automatic refresh remain unavailable. Live Apple sign-in now works in the desktop app: a personal account signed in and its team was listed and selected. Federated company accounts are not supported. Windows remains unverified.

## Run

Install stable Rust, Node.js 22.12+ (Node 24 LTS preferred), and the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/). Development here was verified with Rust 1.98.0, Node 23.5.0, and npm 10.9.2 on Apple Silicon macOS.

```sh
npm ci
npm run tauri dev
```

Choose an IPA or drop one file into the desktop window. The UI reads real metadata through the Rust core. `npm run dev` runs a browser preview; native operations are disabled there. Only real connected-device data is shown; no sample accounts or successful signing results are provided.

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

## Install an already-signed IPA (transport preview)

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

The desktop keeps only the latest job's stage and redacted status in `last-install.json` under its app-data directory. After an interrupted install, check the phone before making a new review. There is no automatic retry, resumable upload, or background refresh. Normal completion/cancellation attempts to remove only its UUID-named staging IPA. Disconnections or process termination may leave that staging file or a local temporary snapshot behind; automatic orphan cleanup is not implemented. Run one Orbiter instance at a time during this preview.

### Local signing identity preview (macOS)

In **Destination & identity**, click **Check Keychain** to list valid iOS signing identities from the current macOS Keychain search list. This is a read-only inventory; it does not export private keys, sign an IPA, contact Apple, or establish profile compatibility. Certificate names are labels, not proof of team authorization. A matching profile for each app/extension and a reviewed signing plan are still required before re-signing can be enabled. Apple account sign-in is available separately as an unvalidated live-account preview. Windows identity discovery is not implemented.


## Apple account sign-in preview — local macOS support

1. Run `npm run tauri dev`. A connected iPhone or IPA is not required.
2. Under **Apple account**, click **Check local support**. The check uses Apple frameworks installed on your Mac and returns status only.
3. Read **Local authentication & Apple communication**, confirm direct Apple authentication, and enter your test account/password in the application only.
4. Choose **Sign in to Apple**, complete trusted-device/SMS verification, and select the intended signing team explicitly.
5. **Refresh teams** checks developer access again. **Sign out** clears the local session. Sessions expire on access after 30 minutes or when the process exits.

No remote Anisette provider or proxy fallback is available. Local support failure stops sign-in. Passwords/codes are never persisted; account sessions remain in memory. macOS manages its own authentication support data. Windows local authentication is not yet implemented. Direct Apple HTTPS access is required, including on corporate networks.

Local generation passed on this Mac; live Apple sign-in still needs a designated test-account acceptance run. No provisioning/certificate/device mutation or re-signing is performed by account sign-in. **Sign & Install** remains unavailable. See [local authentication and data handling](docs/local-authentication.md), including cleanup of any obsolete state from the retired remote preview.
