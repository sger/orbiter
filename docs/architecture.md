# Architecture

```text
React UI ── Tauri commands + progress channel ── Tokio blocking worker
                                                    │
CLI ────────────────────────────────────────── orbiter-core
                                               ├─ ZIP/bundle inspection
                                               ├─ Mach-O signature metadata
                                               ├─ CMS profile payload parsing
                                               └─ conservative findings
```

`crates/orbiter-core` owns inspection and report types and has no Tauri dependency. `src-tauri` owns window setup, inspection, discovery, review, and installation IPC commands, one active inspection, cancellation, and redacted structured logging. `src` renders only returned data and explicit unavailable states. IPC permissions grant only opening a file dialog and registering/removing drag/drop listeners; no shell, arbitrary filesystem frontend access, remote content, or updater permissions. Custom commands expose inspection/discovery, review, and explicitly acknowledged installation. The frontend has no general filesystem or shell permission.

Blocking archive reads run on Tokio's blocking pool. A single atomic busy guard rejects overlapping jobs; a drop guard releases it on success, failure, or worker unwind. Cancellation checks occur between stages, bundles, ZIP entries, and 64 KiB read chunks. CMS/Mach-O/plist parsing is synchronous and cancellation waits for that bounded parse to finish. A late cancellation may lose to completed inspection. No invented byte percentage or immediate-cancellation claim is made. Closing the application abandons inspection; restarting and selecting the original file starts cleanly. Inspection itself has no persistent jobs or resumable partial outputs. The separate installation journal is described below.

## Inspection boundary

All archive entries are inspected for unsafe paths and declared size; only metadata, declared executables, profiles, and candidate icons are decompressed. Resource-file contents and CRCs of unread entries are not verified. No full bundle-signature verification or installability claim is possible from this report.

Limits are intentionally conservative:

| Input | Limit / handling |
| --- | --- |
| IPA | 2 GiB compressed; regular local file |
| ZIP directory | 50,000 entries, 32 MiB directory, single volume; ZIP64/self-extracting/trailing-data variants rejected |
| Expanded archive | 8 GiB declared aggregate; 1 GiB declared per entry; large entries above 1000:1 compression rejected |
| Data actually read | 1 GiB cumulative, one executable at a time; 512 MiB executable |
| Bundles | 512; exactly one direct `Payload/*.app` |
| Plists/profiles | 4 MiB input; plist depth 64, 100,000 events, 8 MiB expanded scalar data |
| Mach-O | At most 32 non-overlapping fat slices; bounded commands and signature slots |
| Icons | 2 MiB standard PNG; no external URLs |

The `plist` streaming API is enabled with an exact package pin so depth/count limits can be applied before recursive `Value` construction. ZIP end-directory preflight bounds allocation before the ZIP library is constructed. Central-directory counts are cross-checked because ZIP libraries may collapse duplicate names. Absolute, parent, backslash, colon, empty, control-character, and trailing-space/dot components are rejected. Symlinked/versioned macOS-style frameworks are intentionally unsupported by this iOS inspector.

The inventory recognizes `.app`, `.appex`, and `.framework` bundle roots with direct Info.plists. Loose dylibs, resource bundles, other nested code layouts, DER entitlement decoding, signature cryptographic verification, and asset-catalog/CgBI icon decoding remain future work. Unknown or malformed executables/profiles produce unverified findings. Unknown capability keys are surfaced for review. Per-slice executable entitlements and provisioning-profile authorizations are separate; an entitlement's presence is not evidence of authorization under a new team.

## Compatibility model

Findings distinguish `not_verified`, `requires_configuration`, and `unsupported`. The UI also defines the future `preserved` label, but no Phase 1 path emits it: there is no selected signing identity. Encryption is an unsupported input for re-signing; expired profiles need configuration. Push, domains, merchants, groups, keychain, Watch identity, and unknown entitlements require review. There is no silent entitlement stripping, identifier rewriting, or Watch/extension removal.

Future signing-plan validation must use explicit target profiles/certificates, inspect every nested code object, compare identifiers and entitlement values, block unsupported capabilities, explain configured changes, and require a reviewed plan before device registration or signing. Tests for such plans, installation job transitions, interrupted-operation recovery, and native credential-store failure must accompany that implementation; empty abstractions/tests have not been added in Phase 1.

## Subsequent boundaries

Add device transport through evaluated `idevice` APIs before authentication. Add an authentication/team service with native credential storage and no plaintext fallback once Anisette data handling is settled. Certificate/provisioning operations, bundle transformation/signing, and installation jobs belong in the Rust core as their behavior becomes concrete. SQLite can persist refresh/job metadata once resumable jobs exist. Signing outputs must use separate paths and preserve the original IPA; never automatically revoke certificates.

## Device discovery (Phase 2, first increment)

`devices.rs` uses exact-pinned idevice 0.1.65 with `usbmuxd` and `ring`; the installation module additionally enables `afc` and `installation_proxy`. Pairing mutation is not enabled. It lists transports through the local daemon, reads selected Lockdown metadata keys, obtains an existing pairing record into memory, and verifies a TLS session. Known non-iPhones are excluded; devices with unavailable metadata remain visible as unverified. Only an ephemeral mux ID, display metadata, connection category, and redacted status leave the core. No UDID, IP address, HostID, or key is exposed in reports/logs.

Discovery has a three-second daemon deadline and three seconds per device, capped at sixteen transports (maximum roughly 51 seconds). The UI skips overlapping polls, refreshes every five seconds when idle, and clears selections that disappear. Stale sessions are not reused. A timeout/disconnection produces an unavailable state. Missing/unreadable pairing records are reported as unverified, rather than claiming that trust was rejected. A successful TLS session establishes pairing; lock state and Developer Mode remain unverified unless the device explicitly returns a locked error. `PasswordProtected` is not used to infer lock state.

The desktop tracing filter admits only Orbiter's structured events, excluding dependency logs that could contain pairing or protocol payloads. The standalone discovery CLI installs no tracing subscriber. Pairing records remain managed by the OS Apple service; Orbiter neither stores nor mutates them. Windows uses loopback only and requires its own hardware validation. Installation transport is implemented as the preview described below; its physical end-to-end validation is pending.

## Existing-signature installation

`installation::prepare` binds an opaque review token to a private local IPA snapshot and the selected phone's UDID, held only in Rust memory. A SHA-256 fingerprint identifies the reviewed bytes. The review expires in ten minutes. Only one prepared review and one review/install operation are admitted per desktop process. The UI pauses discovery/selection while preparing or executing; the backend independently validates the token and acknowledgement and consumes the review once.

Preflight rejects missing/expired profiles, incomplete/encrypted executable inspection, unverified iPhone platform/architecture/OS metadata, and profiles that do not authorize the selected phone (unless `ProvisionsAllDevices` explicitly authorizes all devices). Nested iPhone bundles receive membership checks; Watch bundles stay included, with Watch-device authorization explicitly unverified. No identifier, entitlement, profile, or signature is modified. Installed-app lookup is limited to the main bundle ID. The review shows potential replacement, and execution repeats the lookup and rejects a changed installed version/build.

Before staging, execution checks the same physical UDID, USB connection, pairing, the snapshot's profile conditions, and installed-app state again. AFC transfers in 256 KiB chunks to a UUID-named file under `PublicStaging`; the accumulated hash and byte count must match the review before dispatch. Cancellation is atomic with the transition into installation. After that boundary there is no cancellation or rollback promise. The code uses idevice's InstallationProxy command and progress callback, not the convenience uploader (which reads the entire IPA and uses a shared staging filename).

A job journal is atomically replaced using a private temporary file and synced before device-install dispatch. It records only a random job ID, stage, byte counts, optional iOS-reported percentage, redacted message, and cleanup-needed flag. It records no credentials, UDID, pairing record, source path, or app binary. Preparing/transferring jobs become failed after restart; dispatching/installing jobs become unknown. No job resumes or retries automatically. A reported iOS completion is the sole success signal; 100% progress alone is not success. Explicit device rejections are failed; transport loss after dispatch is unknown.

Individual AFC calls have a 15-second timeout; device connection/lookup have ten-second deadlines; the overall job has a twenty-minute deadline. Normal terminal outcomes attempt to remove only the generated staging file after rechecking the physical device binding. Unknown outcomes leave staging alone because iOS may still be reading it. Cleanup failures are visible. Process-killed temporary snapshots and staging files are not automatically scavenged; the latest-job journal is not a complete refresh database. Atomic replacement is tested locally, but power-loss durability and multi-process coordination are not claimed. Run one desktop process during this preview.
