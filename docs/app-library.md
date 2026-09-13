# Persistent app library

IPAs is the library home. Use Import IPA or drag one or more IPA files onto the library, then search by name or bundle identifier, then open an app to see Versions, Installations, and Devices. Imports are sequential and independent: an invalid file does not stop the remaining imports. Importing never installs anything.

Originals are grouped by their main bundle identifier. SHA-256 identifies bytes, not version labels: identical imports reuse the saved version; different bytes remain separate even if version and build labels match. Versions use import order. Opening an original enters the existing signing workflow; opening a retained signed build goes to installation review. Acknowledgements and device checks still apply.

## Storage and identity

Rust owns `<application-data>/library/manifest.json` (schema 1) and `artifacts/<sha256>.ipa`. Readable embedded PNG app icons are cached as bounded data URLs in the manifest and displayed in library rows and app details. Missing or unreadable icons use a fallback. On macOS, embedded optimized CgBI icons are converted with Apple’s image converter using bounded temporary files and a timeout. Opening an original refreshes its cached library icon. Asset-catalog-only icons are not decoded yet. Import copies into a temporary file in the managed directory, hashes and inspects that copy, flushes it, then atomically publishes the artifact and manifest. Original files are never edited. Metadata mutations are serialized in-process. Invalid or missing manifests with existing artifacts are errors, not permission to create a fresh library.

Apps, artifacts, and attempts have stable identifiers. Source originals and signed artifacts are separate records. Signed records retain their source ID, rewritten bundle identifier, team tag captured inside the signing operation, marker and Watch choices, and known profile expiration. Device records use a SHA-256 tag of a random library-local salt and the verified device identity. Display names are stored separately. Raw UDIDs and account credentials are not stored in the library manifest. The salt stays stable across restarts.

IPC accepts artifact IDs for opening, provisioning, signing, and installation review. Rust resolves and verifies the managed path. Older path-based signing and installation commands resolve existing managed artifacts or import external originals before continuing.

## Operations and history

One React workspace stays mounted while navigating between the library, Settings, and Help. Account state survives navigation. Opening another version invalidates provisioning and installation review. Active account, signing, and installation operations prevent changing the workspace artifact. Backend file leases protect signing, provisioning, and reviewed installation artifacts from removal, including while their page is hidden.

Before installation starts, Rust records an attempt for the exact reviewed hash/artifact and verified device. Stage transitions and terminal results are saved by the backend, independently of page visibility. Recovery uses a matching terminal installation journal; unmatched interrupted attempts become unknown. Failed, cancelled, and unknown outcomes are retained. Success is not inferred from the app name, identifier, elapsed time, or a previous install.

Expiration on a saved original describes its imported profile. Expiration on a signed artifact describes that generated build. An installation row associates expiration with that build only after a recorded successful installation. These records do not establish current presence, trust, launchability, or a full phone inventory. Legacy renewal information remains explicitly labeled and is not converted into invented versions or devices.

## Removal and recovery

Removing a version removes its original and retained signed variants but preserves installation attempts and artifact metadata as historical tombstones. Removing one signed build leaves its original and other variants. Removing an entire app requires explicit confirmation and removes its files and history. None of these actions uninstall anything from a phone.

Removal first atomically records tombstones and pending file cleanup. A failed metadata write leaves files intact. Startup retries only cleanup already requested by a user. Shared content is not deleted while another live record references it. Interrupted publication can leave unreferenced bytes; these are retained, included in storage usage, and never automatically pruned. There is no cloud sync, scheduled renewal, automatic installation queue, or general automatic deletion.

## Verification

Automated coverage includes managed-copy preservation, byte deduplication, same-label versions, missing/damaged artifacts, failed metadata writes, corrupt manifests, deletion leases, deletion recovery, stable salted device matching, multiple signed variants/devices, exact attempt attribution, and interrupted-job recovery. Browser tests cover the library screens, partial import failure, saved workspace navigation, explicit installation review, retained-build history, removal confirmation, restart persistence in the IPC test double, and light/dark keyboard layouts at 780 × 600.

Browser tests use synthetic IPC. Rust tests use temporary files and synthetic IPAs. Neither substitutes for a real phone. The new library flow still requires this physical-device acceptance check:

1. Import a real IPA; verify the source file remains unchanged.
2. Open it, authenticate, explicitly prepare signing, and sign. Confirm a retained signed artifact appears under the original.
3. Review that artifact for an unlocked USB iPhone, acknowledge, and install. Confirm artifact/device/outcome attribution.
4. Restart Orbiter. Reopen the same saved artifact and verify metadata, history, device tag, and expiration persist.
5. Review a second installation independently; do not assume the prior install authorizes another.

Do not claim native end-to-end validation of the library until this check has been completed.
