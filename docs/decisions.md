# Dependency and platform decisions

Public sources rechecked on 2026-09-12. Source inspection is not an API integration or platform test. No proprietary Sideloadly binary, code, branding, or investigation artifact is bundled.

## Current stack

Use Tauri 2 and React/TypeScript for the desktop UI, Tokio for off-thread work, and an independent Rust core. Parsing uses `zip` with deflate only, `plist`, and RustCrypto `cms`/`der`. CMS decoding is portable and requires neither the macOS `security` executable nor OpenSSL. The small Mach-O inspector reads only bounded headers, load commands, and XML entitlement blobs; this is not signing-protocol implementation.

Direct versions are exact and both Cargo/npm lockfiles are tracked. The chosen stable versions build locally; Vite 7 is retained because it supports this host's Node version. Orbiter uses isideload's vendored authentication and portal APIs, and calls the `apple-codesign` fork it resolves directly from `signer.rs` — one copy of the signing implementation in the graph rather than two. Read-only discovery now uses exact-pinned `idevice` 0.1.65 with `usbmuxd` and `ring`, plus `afc`/`installation_proxy` for the unchanged-IPA installation preview. The repository manifest says 0.1.66, but that version was not published when queried; the downloaded 0.1.65 API was compiled and exercised.

## Evaluated projects

| Project | Current observations | Decision |
| --- | --- | --- |
| [iloader](https://github.com/nab138/iloader) | Tauri/Rust application using isideload/idevice. Source is MIT; branding has [separate restrictions](https://github.com/nab138/iloader/blob/main/LICENSE-BRANDING). README still lists multi-team selection and refresh among future plans. | Workflow/API reference only. Orbiter has independent visuals. Do not infer company-IPA compatibility. |
| [isideload](https://github.com/nab138/isideload) | Inspected manifest: 0.3.17, MIT. Requires `isideload::init()` for detailed network errors. Features include install and keyring storage; its README explicitly leaves proper entitlement handling unfinished. Uses prerelease crypto dependencies and the MPL signing fork below. | Adopted exact 0.3.17 for the local authentication/team preview. No plaintext storage feature or upstream signer is used. |
| [idevice](https://github.com/jkcoxson/idevice) | MIT; development/research status and breaking point-release APIs. `UsbmuxdConnection`, device providers, `LockdowndClient`, and features for `usbmuxd`, `pair`, `afc`, and `installation_proxy` are relevant. isideload's inspected manifest depends on 0.1.61. | Adopted 0.1.65 for discovery and the installation-transport preview. Compiled APIs use `LockdownClient` (not the README spelling) and `to_provider(addr, label)`. Existing-session verification works on the connected macOS iPhone; Windows and physical interruption/recovery behavior remain unverified; the user confirmed the native installation happy path. |
| [isideload-apple-platform-rs](https://github.com/nab138/isideload-apple-platform-rs) | `isideload-apple-codesign` manifest 0.29.11 declares MPL-2.0 and a substantial cryptographic/bundle dependency graph. | Evaluate nested signing and verification on synthetic bundles before adoption. MIT at the top level does not cover the entire dependency graph. |
| [Tauri 2](https://v2.tauri.app/start/prerequisites/) | Native webviews and native toolchains; Windows requires MSVC tools and WebView2. | Current desktop shell with narrow IPC permissions. Native build and testing still required per OS. |

## Authentication support and data flows

The initial remote-provider preview was rejected by the user for company data handling. It has been removed. Current account authentication uses a local macOS provider with a patched isideload adapter that requires explicit provider selection and direct Apple HTTPS. No remote support service or proxy fallback is permitted. See [local authentication](local-authentication.md) and [the vendored patch notes](../vendor/isideload/ORBITER-PATCHES.md). Native generation passed on the current Mac; live account acceptance and Windows local support remain pending.

## Apple limits and team identity

Apple's [device overview](https://developer.apple.com/help/account/devices/devices-overview/) currently specifies 100 registered devices per product family per membership year, and disabling a device does not restore a slot during the year. A different account selecting the same company team uses that team's resources and allowance; it does not create a separate quota.

Apple's [developer account overview](https://developer.apple.com/help/account/basics/about-your-developer-account) documents Personal Team management in Xcode, up to 10 App IDs and 3 registered devices with seven-day expiry, up to 3 installed apps per device, and seven-day provisioning profiles. Personal teams have capability restrictions. These are not guarantees that third-party authentication can provision this IPA. Recheck requirements and the [supported iOS capabilities](https://developer.apple.com/help/account/reference/supported-capabilities-ios) before implementation.

An IPA's supported device families and a profile's device list are metadata/snapshots, not current portal inventory. Neither establishes remaining quota or a bypass. This tool does not promise to bypass device registration limits.

## Licenses

`dependencies-aarch64-apple-darwin.json` records the resolved host Cargo graph and npm lockfile metadata. Regenerate with `python3 scripts/dependency-inventory.py`; pass a Rust target triple for another platform. This includes build/test dependencies and optional npm platform packages, not only shipped code.

The host metadata includes MIT/Apache alternatives, BSD, Zlib, Unicode, and MPL-2.0. The MPL components include HTML/CSS helpers and an option utility (`cssparser`, `cssparser-macros`, `dtoa-short`, `selectors`, `option-ext`). Orbiter now invokes the unmodified MPL-licensed signing fork (`isideload-apple-codesign`) directly: it is shipped code, not merely a transitive dependency, and its notice obligations apply in full. Preserve required notices, review MPL file-level obligations and any modified covered files, and generate actual distribution notices/license texts before release. Package metadata is an initial transitive review, not legal approval or a completed distribution license bundle. Windows-specific Cargo dependencies still need their own resolved inventory and review. First-party repository licensing has not been chosen on behalf of the company.

The discovery increment adds idevice/ring/TLS dependencies to the generated host license inventory. Published metadata declares idevice MIT, ring Apache-2.0 AND ISC (both notice sets), rustls Apache-2.0 OR ISC OR MIT, and untrusted ISC. This was true for the discovery increment; the local account preview now links the authentication adapter. The existing no-plaintext/no-secret-logging rules also apply to device pairing material; upstream verbose protocol logs are filtered out.

## Existing-signature transport decision

Apple's [Ad Hoc workflow](https://developer.apple.com/help/account/provisioning-profiles/create-an-ad-hoc-provisioning-profile) requires a matching App ID, distribution certificate, and registered devices. The new flow uses that existing authorization; it does not register devices or promise a quota bypass. Runtime trust remains iOS's decision.

The downloaded idevice 0.1.65 source provides AFC chunk writes and InstallationProxy `get_apps`/`install_with_callback`. Its high-level uploader uses a shared `PublicStaging/idevice.ipa` path and whole-file buffering. Orbiter uses the lower-level APIs for unique staging paths, bounded transfers, review-bound hashing, and cancellation. Inspection of the library's completion loop confirmed that success requires `Status=Complete`; device `Error` values are converted into errors before that loop. Raw device error descriptions and dependency logs are not exposed by Orbiter.

`tempfile`, SHA-256 (`sha2`), and UUID v4 support private snapshots, reviewed-byte identity, atomic journal replacement, and staging ownership. The dependency inventory was regenerated for the enabled installation features. These are not Apple signing or authentication libraries.

The installation additions declare MIT (async_zip) or MIT/Apache alternatives (async-compression, compression-codecs, sha2, tempfile, uuid). No missing license metadata was found in the regenerated host inventory; Windows-specific review and release notice assembly remain pending.

## Local authentication decision — 2026-09-12

The user explicitly selected local support plus direct Apple communication. The former public service is removed from the active provider and UI. Upstream 0.3.17 is vendored under its MIT license with narrow changes: a local-data constructor, removal of the remote provider/default, Apple HTTPS destination validation, and disabled proxies/redirects. Authentication cryptography is unchanged. MacAnisette's MIT-licensed selector reference informed the local bridge; its notice is included. No Apple binary is bundled. Private framework availability on future macOS versions and a Windows local provider remain open.

Dependency/license review: isideload is MIT; the optional keyring-storage feature is disabled in the local-only adapter. Its `install` feature is required by unconditional idevice imports in this version even for account-only use. Its signing crates are not feature-gated and introduce seven MPL-2.0 packages: apple-bundles, apple-codesign, apple-flat-package, apple-xar, cpio-archive, cryptographic-message-syntax, and x509-certificate (all isideload-prefixed). They are unmodified and must be covered by release notices/source-availability obligations. New graph metadata also includes CDLA-Permissive-2.0 trust-root data, bzip2-1.0.6, Zlib, BSD, and MIT/Apache alternatives; no missing license metadata was found. The host inventory was regenerated. Windows inventory, distribution notices, and a release review remain pending. Prerelease crypto dependencies remain locked; successful compilation is not a cryptographic audit.


## Product requirement: testers beyond the company device allowance — 2026-09-12

The company's paid team has reached Apple's 100-device-per-product-family limit for the membership year, so tester devices cannot be added to the company provisioning profile. Orbiter exists to put the company build on those devices anyway, by re-signing it with each tester's own free Apple account: the tester's device is registered on their own Personal Team, which draws on that team's allowance rather than the company's. This is the driving requirement for Phase 3 and it settles several open design questions.

Consequences, each of which is a design constraint rather than a detail:

- **Re-signing is mandatory, not optional.** The existing-signature installation path works only for devices already in the company profile — precisely the devices this requirement excludes. Every tester install needs a new identifier, certificate, App ID, and profile on the tester's Personal Team.
- **Identifier rewriting becomes a required feature.** The company bundle identifiers belong to the company team and cannot be claimed by a Personal Team, so identifiers must be rewritten, and the rewrite cascades into app groups, keychain access groups, and each nested bundle's parent identifier. The existing "no silent identifier rewriting" rule stands as *no silent* rewriting: the plan must state every changed identifier and be acknowledged.
- **Capabilities the Personal Team cannot create must be dropped, and the build is then degraded.** Apple's published capability table does not cleanly separate Personal Team support, and two readings of it disagreed, so the supported set must be established from what the portal actually accepts when creating the App ID and profile, not from a table compiled into Orbiter. The representative IPA declares push, associated domains, Apple Pay, app groups, and keychain access; any of these that the Personal Team refuses must be removed explicitly, with the runtime consequence stated to the tester, and the review must be able to block instead.
- **Seven-day expiry makes refresh a core feature, not a later milestone.** Personal Team profiles expire in seven days, so every tester needs a repeatable weekly re-sign. Documented Personal Team limits also cap 10 App IDs per seven days, 3 registered devices, and 3 installed apps per device.
- **The representative IPA needs two App IDs, not twenty-four.** Its 24 bundles are the main app, one Watch app, and 22 frameworks; frameworks are signed but do not consume App IDs. The Watch app does, and Watch provisioning under a Personal Team is unverified, so removing it may be the only way to install — again explicitly, never silently.
- **macOS-only local authentication is a blocker for Windows testers.** Sign-in requires the local macOS authentication bridge. If testers run Windows, no part of this workflow is available to them until a Windows local provider exists, and no remote support server is permitted.

The conventional answers to a full device allowance are TestFlight, whose external testers do not consume device slots, and the Apple Developer Enterprise Program for in-house distribution; the 100-device limit also resets at membership renewal. These were not selected. This requirement is recorded as the company's decision, and Orbiter does not claim to bypass Apple device limits: it uses each tester's own Personal Team allowance, with that team's restrictions and expiry.


## Observed team membership names

A live free Apple account reports its Personal Team as type `Individual` with the membership name **"Xcode Free Provisioning Program"**. The name ends in "Program" like a paid membership, so classification matches the free wording first and only then the paid programs, and a Personal Team that reports no membership at all is also treated as free. Anything else stays undetermined and is shown as such, including what Apple actually reported, rather than being guessed: the free-versus-paid answer decides seven-day expiry and which capabilities survive re-signing. This string was read from one account and may vary; the undetermined path is the safety net.
