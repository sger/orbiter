# Releasing

A `v*` tag builds an Apple Silicon DMG and attaches it to a **draft** GitHub release, via [`.github/workflows/release.yml`](../.github/workflows/release.yml).

```sh
# The version lives in src-tauri/tauri.conf.json. Bump it first.
git tag v0.2.0
git push origin v0.2.0
```

The workflow runs the same checks CI does, builds, and uploads the DMG with a `.sha256` beside it. It stops at a draft, marked pre-release, on purpose: the physical-device checks in [validation.md](validation.md) cannot run on a CI runner, so nothing signs off on a build but a person. Download the draft's DMG, work through those checks, then publish.

The pre-release flag is about the software, not the pipeline: the two-factor path has never run against Apple, installation interruption and recovery are unvalidated, and Windows is unverified. Drop the flag on a later tag once those are exercised — no renumbering needed.

`workflow_dispatch` runs the same build without publishing anything, for checking the pipeline itself.

## Unsigned, and what that costs

Orbiter is not signed with an Apple Developer ID. That certificate is only issued to paid Apple Developer Program members, and Orbiter exists for people who do not have one.

The cost lands on whoever downloads it. macOS refuses the first launch, saying it cannot check the app for malicious software, and the way past it is:

**System Settings → Privacy & Security → Security → Open Anyway**

Then open the app again and confirm. Only the first launch needs it. The release notes say this, because a download that fails to open with no explanation is worse than one that warns you first.

The `.sha256` is the only thing a person can actually verify about an unsigned build, so it ships with every release.

Some managed Macs go further: endpoint security software can block or quarantine an unsigned app outright, with no "Open Anyway" available. There is no fix for that short of signing.

## If you ever do sign

Two things change, both small:

1. Set `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` and `APPLE_SIGNING_IDENTITY` as repository secrets, plus either an Apple ID group (`APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`) or an API key group (`APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH`) for notarization. Tauri picks up all of these from the environment during `tauri build`.
2. Add a verification step after the build — `codesign --verify --deep --strict`, `spctl --assess --type execute`, and `xcrun stapler validate` on both the app and the DMG — so a broken signature fails the release rather than shipping.

Notarization is not App Store distribution: Apple scans the build for malware and returns a ticket. No store listing, no review.

## Apple Silicon only

[validation.md](validation.md) records Apple Silicon as the tested host, so that is what is built, and `minimumSystemVersion` is 11.0 because no Apple Silicon Mac runs anything older.

Intel means setting `TARGET` to `universal-apple-darwin` and lowering `minimumSystemVersion`. Do that once someone has actually run Orbiter on an Intel Mac — an untested slice is a support claim, not a build setting.

## Hardened runtime

Only relevant if signing is added later, but already checked. The hardened runtime restricts a process to loading libraries signed by Apple or the same team, and Orbiter loads AOSKit and AuthKit from `/System/Library/PrivateFrameworks` at runtime and writes its signing key to the Keychain.

Both work under an ad-hoc signature with `--options runtime`, which applies the same restrictions, so **no entitlements file is needed**. To repeat the check:

```sh
cargo test --locked -p orbiter-core --lib --no-run
codesign --force --options runtime --sign - target/debug/deps/orbiter_core-*
target/debug/deps/orbiter_core-* local_anisette:: --ignored
target/debug/deps/orbiter_core-* keychain:: --ignored
```

If a future macOS changes that, `com.apple.security.cs.disable-library-validation` is the escape hatch. Adding it before something breaks would weaken the build for nothing.

## Not verified

This workflow has never run. What is established locally: the bundle builds for `aarch64-apple-darwin`, `LSMinimumSystemVersion` is 11.0, and neither native path breaks under the hardened runtime.
