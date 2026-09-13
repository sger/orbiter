<p align="center">
  <img src="public/orbiter.png" width="128" height="128" alt="Orbiter logo" />
</p>

# Orbiter

An open-source desktop app for managing IPAs and installing apps on your iPhone. Built with Rust, Tauri, and React.

- Import IPAs with drag and drop and keep multiple versions in your app library.
- Install an already-signed IPA or re-sign a copy with your Apple account.
- Review installation history and known profile expiration dates.
- Keep your library locally, with light and dark themes.

Requires an Apple Silicon Mac on macOS 11 or later. Windows support is unverified.

## Install

Download the latest DMG from [Releases](https://github.com/sger/orbiter/releases), open it, and drag Orbiter to Applications.

Orbiter is not signed with an Apple Developer ID, so macOS blocks the first launch and says it cannot check the app for malicious software. To allow it:

1. Open Orbiter and dismiss the warning.
2. Go to **System Settings → Privacy & Security**, scroll to **Security**, and click **Open Anyway** beside the message about Orbiter.
3. Open Orbiter again and confirm.

Only the first launch needs this. Each release ships a `.sha256` file if you want to check the download.

## Build from source

Install [Rust](https://www.rust-lang.org/tools/install), Node.js 22.12 or later, and the [Tauri macOS prerequisites](https://v2.tauri.app/start/prerequisites/#macos).

```sh
git clone https://github.com/sger/orbiter.git
cd orbiter
npm ci
npm run tauri build -- --bundles app
```

Copy `target/release/bundle/macos/Orbiter.app` into Applications and open it. A build made on your own Mac opens without the step above.

## Install an app on your iPhone

1. Connect your iPhone by USB, unlock it, and establish trust in Finder.
2. Open **App Library** and choose **Install an app**. Import an IPA or select a saved version.
3. Select your iPhone and run the checks. Sign in with your Apple account only if re-signing is needed or you choose it.
4. Review the requested changes and installation details, then confirm installation.

Original IPAs stay unchanged. Passwords are never saved. Re-signing can affect app capabilities, and Apple account limits still apply. Expiration dates do not confirm that an app is currently installed or working.

## Development

```sh
npm ci
npm run tauri dev
```

For a browser-only UI preview, use `npm run dev`; device and signing operations require the desktop app.

```sh
npm run build
npm test # Requires Google Chrome
cargo test --locked -p orbiter-core
cargo clippy --locked --workspace --all-targets -- -D warnings
```

Issues and pull requests are welcome.

## Documentation

[Installation flow](docs/guided-installation.md) · [App library](docs/app-library.md) · [Architecture](docs/architecture.md) · [Data handling](docs/local-authentication.md) · [Validation status](docs/validation.md) · [Releasing](docs/release.md)
