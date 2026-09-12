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

`crates/orbiter-core` owns inspection and report types and has no Tauri dependency. `src-tauri` owns window setup, two IPC commands, one active inspection, cancellation, and redacted structured logging. `src` renders only returned data and explicit unavailable states. IPC permissions grant only opening a file dialog and registering/removing drag/drop listeners; no shell, arbitrary filesystem frontend access, remote content, or updater permissions. Custom commands expose inspection and cancellation, not file writes.

Blocking archive reads run on Tokio's blocking pool. A single atomic busy guard rejects overlapping jobs; a drop guard releases it on success, failure, or worker unwind. Cancellation checks occur between stages, bundles, ZIP entries, and 64 KiB read chunks. CMS/Mach-O/plist parsing is synchronous and cancellation waits for that bounded parse to finish. A late cancellation may lose to completed inspection. No invented byte percentage or immediate-cancellation claim is made. Closing the application abandons inspection; restarting and selecting the original file starts cleanly. There are no persistent installation jobs or resumable partial outputs.

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
