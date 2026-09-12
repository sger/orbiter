# Dependency and platform decisions

Public sources rechecked on 2026-09-12. Source inspection is not an API integration or platform test. No proprietary Sideloadly binary, code, branding, or investigation artifact is bundled.

## Current stack

Use Tauri 2 and React/TypeScript for the desktop UI, Tokio for off-thread work, and an independent Rust core. Parsing uses `zip` with deflate only, `plist`, and RustCrypto `cms`/`der`. CMS decoding is portable and requires neither the macOS `security` executable nor OpenSSL. The small Mach-O inspector reads only bounded headers, load commands, and XML entitlement blobs; this is not signing-protocol implementation.

Direct versions are exact and both Cargo/npm lockfiles are tracked. The chosen stable versions build locally; Vite 7 is retained because it supports this host's Node version. No signing or device dependency is adopted yet.

## Evaluated projects

| Project | Current observations | Decision |
| --- | --- | --- |
| [iloader](https://github.com/nab138/iloader) | Tauri/Rust application using isideload/idevice. Source is MIT; branding has [separate restrictions](https://github.com/nab138/iloader/blob/main/LICENSE-BRANDING). README still lists multi-team selection and refresh among future plans. | Workflow/API reference only. Orbiter has independent visuals. Do not infer company-IPA compatibility. |
| [isideload](https://github.com/nab138/isideload) | Inspected manifest: 0.3.17, MIT. Requires `isideload::init()` for detailed network errors. Features include install and keyring storage; its README explicitly leaves proper entitlement handling unfinished. Uses prerelease crypto dependencies and the MPL signing fork below. | Candidate for a controlled authentication spike, not a drop-in company signing pipeline. Disable any plaintext storage option and audit logging/default services before adoption. |
| [idevice](https://github.com/jkcoxson/idevice) | MIT; development/research status and breaking point-release APIs. `UsbmuxdConnection`, device providers, `LockdowndClient`, and features for `usbmuxd`, `pair`, `afc`, and `installation_proxy` are relevant. isideload's inspected manifest depends on 0.1.61. | Preferred device spike. Pin an exact tested release; verify actual compiled APIs rather than copying potentially stale README examples. |
| [isideload-apple-platform-rs](https://github.com/nab138/isideload-apple-platform-rs) | `isideload-apple-codesign` manifest 0.29.11 declares MPL-2.0 and a substantial cryptographic/bundle dependency graph. | Evaluate nested signing and verification on synthetic bundles before adoption. MIT at the top level does not cover the entire dependency graph. |
| [Tauri 2](https://v2.tauri.app/start/prerequisites/) | Native webviews and native toolchains; Windows requires MSVC tools and WebView2. | Current desktop shell with narrow IPC permissions. Native build and testing still required per OS. |

## Authentication support and data flows

The inspected [Anisette module](https://github.com/nab138/isideload/blob/main/isideload/src/anisette/mod.rs) defines an `AnisetteProvider` boundary. The [remote v3 implementation](https://github.com/nab138/isideload/blob/main/isideload/src/anisette/remote_v3/mod.rs) defaults to `https://ani.stikstore.app`. It posts a base64 keychain identifier and `adi_pb` to `/v3/get_headers`, exchanges provisioning messages over `/v3/provisioning_session` WebSocket, and persists Anisette state through the configured storage implementation. Apple provisioning requests are made through GrandSlam; provisioning responses are relayed to the support service. An optional proxy adds another possible recipient.

This makes the remote service a recipient of persistent authentication-support material and ordinary network metadata. Source review does not establish its operator retention policy, security, availability, or behavior in production. No claim is made that the support service receives an Apple password; the reviewed provider is not a full authentication trace.

**No authentication implementation or service is selected or contacted by Orbiter.** Before integration, compare on-device/local Anisette generation and its native-library licensing/platform requirements with a reviewed remote service. A local option's macOS/Windows feasibility has not yet been established. Document exact recipients, payloads, storage, retention, and proxy behavior in the application; obtain user review before those flows. Use OS credential storage for secrets with no plaintext fallback. Apple credentials must not be sent to company infrastructure.

## Apple limits and team identity

Apple's [device overview](https://developer.apple.com/help/account/devices/devices-overview/) currently specifies 100 registered devices per product family per membership year, and disabling a device does not restore a slot during the year. A different account selecting the same company team uses that team's resources and allowance; it does not create a separate quota.

Apple's [developer account overview](https://developer.apple.com/help/account/basics/about-your-developer-account) documents Personal Team management in Xcode, up to 10 App IDs and 3 registered devices with seven-day expiry, up to 3 installed apps per device, and seven-day provisioning profiles. Personal teams have capability restrictions. These are not guarantees that third-party authentication can provision this IPA. Recheck requirements and the [supported iOS capabilities](https://developer.apple.com/help/account/reference/supported-capabilities-ios) before implementation.

An IPA's supported device families and a profile's device list are metadata/snapshots, not current portal inventory. Neither establishes remaining quota or a bypass. This tool does not promise to bypass device registration limits.

## Licenses

`dependencies-aarch64-apple-darwin.json` records the resolved host Cargo graph and npm lockfile metadata. Regenerate with `python3 scripts/dependency-inventory.py`; pass a Rust target triple for another platform. This includes build/test dependencies and optional npm platform packages, not only shipped code.

The host metadata includes MIT/Apache alternatives, BSD, Zlib, Unicode, and MPL-2.0. The MPL components include HTML/CSS helpers and an option utility (`cssparser`, `cssparser-macros`, `dtoa-short`, `selectors`, `option-ext`). No signing fork is currently included. Preserve required notices, review MPL file-level obligations and any modified covered files, and generate actual distribution notices/license texts before release. Package metadata is an initial transitive review, not legal approval or a completed distribution license bundle. Windows-specific Cargo dependencies still need their own resolved inventory and review. First-party repository licensing has not been chosen on behalf of the company.
