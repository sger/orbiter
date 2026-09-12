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

Add device transport through evaluated `idevice` APIs before authentication. The authentication/team preview now uses in-memory sessions and the local macOS Anisette flow described below. Certificate/provisioning operations, bundle transformation/signing, and installation jobs belong in the Rust core as their behavior becomes concrete. SQLite can persist refresh/job metadata once resumable jobs exist. Signing outputs must use separate paths and preserve the original IPA; never automatically revoke certificates.

## Device discovery (Phase 2, first increment)

`devices.rs` uses exact-pinned idevice 0.1.65 with `usbmuxd` and `ring`; the installation module additionally enables `afc` and `installation_proxy`. Pairing mutation is not enabled. It lists transports through the local daemon, reads selected Lockdown metadata keys, obtains an existing pairing record into memory, and verifies a TLS session. Known non-iPhones are excluded; devices with unavailable metadata remain visible as unverified. Only an ephemeral mux ID, display metadata, connection category, and redacted status leave the core. No UDID, IP address, HostID, or key is exposed in reports/logs.

Discovery has a three-second daemon deadline and three seconds per device, capped at sixteen transports (maximum roughly 51 seconds). The UI skips overlapping polls, refreshes every five seconds when idle, and clears selections that disappear. Stale sessions are not reused. A timeout/disconnection produces an unavailable state. Missing/unreadable pairing records are reported as unverified, rather than claiming that trust was rejected. A successful TLS session establishes pairing; lock state and Developer Mode remain unverified unless the device explicitly returns a locked error. `PasswordProtected` is not used to infer lock state.

The desktop tracing filter admits only Orbiter's structured events, excluding dependency logs that could contain pairing or protocol payloads. The standalone discovery CLI installs no tracing subscriber. Pairing records remain managed by the OS Apple service; Orbiter neither stores nor mutates them. Windows uses loopback only and requires its own hardware validation. Installation transport is implemented as the preview described below; its happy path was confirmed by the user; interruption/recovery validation is pending.

## Existing-signature installation

`installation::prepare` binds an opaque review token to a private local IPA snapshot and the selected phone's UDID, held only in Rust memory. A SHA-256 fingerprint identifies the reviewed bytes. The review expires in ten minutes. Only one prepared review and one review/install operation are admitted per desktop process. The UI pauses discovery/selection while preparing or executing; the backend independently validates the token and acknowledgement and consumes the review once.

Preflight rejects missing/expired profiles, incomplete/encrypted executable inspection, unverified iPhone platform/architecture/OS metadata, and profiles that do not authorize the selected phone (unless `ProvisionsAllDevices` explicitly authorizes all devices). Nested iPhone bundles receive membership checks; Watch bundles stay included, with Watch-device authorization explicitly unverified. No identifier, entitlement, profile, or signature is modified. Installed-app lookup is limited to the main bundle ID. The review shows potential replacement, and execution repeats the lookup and rejects a changed installed version/build.

Before staging, execution checks the same physical UDID, USB connection, pairing, the snapshot's profile conditions, and installed-app state again. AFC transfers in 256 KiB chunks to a UUID-named file under `PublicStaging`; the accumulated hash and byte count must match the review before dispatch. Cancellation is atomic with the transition into installation. After that boundary there is no cancellation or rollback promise. The code uses idevice's InstallationProxy command and progress callback, not the convenience uploader (which reads the entire IPA and uses a shared staging filename).

A job journal is atomically replaced using a private temporary file and synced before device-install dispatch. It records only a random job ID, stage, byte counts, optional iOS-reported percentage, redacted message, and cleanup-needed flag. It records no credentials, UDID, pairing record, source path, or app binary. Preparing/transferring jobs become failed after restart; dispatching/installing jobs become unknown. No job resumes or retries automatically. A reported iOS completion is the sole success signal; 100% progress alone is not success. Explicit device rejections are failed; transport loss after dispatch is unknown.

Individual AFC calls have a 15-second timeout; device connection/lookup have ten-second deadlines; the overall job has a twenty-minute deadline. Normal terminal outcomes attempt to remove only the generated staging file after rechecking the physical device binding. Unknown outcomes leave staging alone because iOS may still be reading it. Cleanup failures are visible. Process-killed temporary snapshots and staging files are not automatically scavenged; the latest-job journal is not a complete refresh database. Atomic replacement is tested locally, but power-loss durability and multi-process coordination are not claimed. Run one desktop process during this preview.

## Local signing identity inventory

`signing::discover` runs the fixed macOS `security find-identity -v -p codesigning` command on explicit UI request, with null stdin/stderr, bounded stdout, a deadline, and child termination on drop. It filters certificate labels to Apple Development/Distribution and legacy iPhone equivalents. It returns public certificate fingerprints and labels only, without inferring team IDs. It neither exports nor exercises private keys. Identities are held in frontend memory and cleared before refresh; they are not logged or persisted. The separate Tauri command performs no signing or account mutation. Windows reports this adapter unavailable. Actual signing must revalidate identity and certificate/profile compatibility rather than trusting this inventory.


## Account authentication and team selection

`accounts::Accounts` owns one active attempt or in-memory session per process. Consent/input validation precedes work. Login has a ten-minute deadline; every 2FA prompt has an opaque one-use ID and masked phone labels. Sign-out invalidates the generation and aborts pending login. Late results cannot restore a signed-out account. Team selection is explicit and must belong to the current session. Refresh is serialized and limited to 60 seconds; failure clears account/team state. The local 30-minute expiry is checked on status and account operations. No credentials or tokens are persisted by Orbiter.

The explicit LocalProvider resolves macOS authentication material before the account builder contacts Apple. A bounded Objective-C bridge links only Foundation and dynamically loads installed Apple frameworks. No authentication-support server or remote-provider fallback exists. The vendored upstream adapter enforces direct Apple HTTPS destinations, disables redirects/proxies/debug TLS, and exposes a constructor for local data. See [local-authentication.md](local-authentication.md) for API, privacy, timeout, licensing, and platform limits. Windows authentication returns unavailable. Existing-signature installation and Keychain identity inventory remain independent of account sign-in.


## Re-signing plan

`plan.rs` computes what re-signing an inspected IPA under another team would change. It is pure local computation: it signs nothing, writes nothing, and contacts no Apple service. Identifiers are rewritten deterministically from the target team ID, so the same team always produces the same identifiers and a weekly re-sign replaces the tester's app instead of installing a second copy. Nested bundles keep their relationship to the main app's new identifier; frameworks are rewritten but consume no App ID.

Each team-scoped entitlement becomes a decision carrying its action, its reason, and the runtime consequence for the tester: push, associated domains, Apple Pay merchant identifiers, and app groups are proposed for removal under a Personal Team, keychain groups and application/team identifiers are rewritten, and the development entitlement is set. Every such decision is marked as needing portal confirmation, because Apple's published capability table does not cleanly separate Personal Team support; the portal's answer must override the proposal once that work exists. Unrecognised `com.apple.developer.*` keys are proposed for removal rather than silently kept.

Encrypted executables, a missing main identifier, an unselected team, and exceeding a Personal Team's ten App IDs per seven days are blockers. A Watch app is never removed silently, and never kept silently either: while the choice is open it is a blocker, because registering its identifier spends one of a Personal Team's ten per seven days and Watch provisioning under such a team is unverified. `WatchChoice::Remove` drops the Watch app and every bundle nested inside it and states that its watchOS features are gone; `WatchChoice::Sign` keeps it and states that it may fail to install or run. Either way the outcome is a stated consequence of the plan, not an open question in the interface. Consequences are collected for acknowledgement, including the seven-day expiry that makes a weekly re-sign necessary.

The plan is read-only output today, with no signer behind it:

```sh
cargo run --locked -p orbiter-core --bin orbiter-sign-plan -- /path/to/company.ipa TEAMID --personal
```

It exits non-zero when the plan has blockers. Actual signing must revalidate every decision against the real certificate, App ID, and profile rather than trusting this plan.


## Device registration

`provisioning::register` is the first Orbiter operation that writes to Apple rather than reading. It refuses before any request unless a live session exists, a team is selected, and the consequence has been acknowledged: a free personal team allows three devices, and a paid team consumes one of its 100 slots for the membership year, which removing the device later does not return. The refusal order is checked by tests, and a view that merely says "signed in" is not enough — the session object itself authorises the write, so a stale or forged view cannot reach Apple.

The device identifier is read through the existing verified-iPhone path in the installation module, which requires USB, a readable pairing record, a verified session, and a real iPhone. It is handed straight to Apple with the device name, and never enters the account view, the report, the journal, or any log; the returned outcome carries only whether the device was already registered or was registered now, and how many devices the team then has. A team membership whose kind Apple did not establish is treated as the stricter free allowance rather than the permissive one.

Registration lists the team's devices first, so an already-registered iPhone writes nothing. A timeout during the write says explicitly that the request may still have been applied and to check developer.apple.com, because a portal write has no rollback. The UI offers registration only once a team is selected, clears its result when the device or team changes, and requires the acknowledgement checkbox each time.


## Development certificate

`certificates::ensure` obtains the certificate the signer will use. The RSA key is generated on this Mac and never leaves it: only a PKCS#10 request goes to Apple, and its subject carries no account, device, or company information — a test asserts that and that the key is 2048-bit. Key generation is CPU-bound and runs off the async runtime.

The team's certificates are listed first and matched against the key they certify, not their name, so a rerun with the same session reuses the existing certificate and writes nothing. Reaching Apple's active-certificate maximum is reported with the count and left to the person: Orbiter never revokes, because revoking invalidates every app already signed with that certificate, including apps Orbiter did not produce. A timeout during the request says the certificate may still have been issued and to check developer.apple.com, since there is no rollback.

The key and certificate live in the signed-in session in memory and are not persisted, so restarting Orbiter needs a new certificate. That is a real limitation for the weekly refresh cycle, given the small number of active certificates a team allows, and persistence is deliberately left as its own decision rather than quietly writing a private key somewhere. Apple labels the certificate with the computer name, bounded to plain text with a neutral fallback.


## App identifiers and profiles

`provisioning::ensure_app_id` registers the plan's rewritten identifiers on the selected team, reusing any the team already holds so a rerun writes nothing. Apple's remaining-identifier count is reported, and a team with none left is refused with what to do about it rather than attempting a write. Registration is acknowledged first: a free personal team may register only ten identifiers per seven days, and an identifier cannot be reused by another team afterwards.

This step is where Apple answers the capability questions the plan can only propose. The created App ID reports which features Apple actually enabled, and those are shown as labels — the few feature keys whose meaning is established by use are named, and anything else is reported as an unnamed capability rather than guessed. A capability Apple reports as disabled is not presented as present.

`provisioning::fetch_profile` then downloads the team provisioning profile for each identifier, which authorises the team's registered devices and, on a free personal team, expires in seven days. Profile bytes are kept in the signed-in session for the signer and are deliberately not serialised into the interface; only the identifier, expiry, and UUID are shown.

Provisioning runs the plan first and writes nothing at all when the plan has blockers. It is driven from the selected IPA, so changing the IPA, the device, or the team clears the result rather than carrying it across.

## The signer

`signer.rs` turns a reviewed plan, Apple's profiles, and this Mac's certificate into a new IPA. The IPA the person selected is opened read-only and never written; everything happens in a temporary directory inside the application's own storage, which is removed when the operation returns, and the result is a separate file named for the team it was signed for.

Order of work: extract the archive under the same refusals inspection applies (unsafe paths, symlinks, special files, case-colliding duplicates, 2 GiB / 50,000 entries / 8 GiB expanded); remove every bundle the plan left out, shallowest first, so a removed Watch app takes its extensions and frameworks with it and is reported once; rewrite identifiers; install one profile per bundle that holds an App ID; sign from the inside out; repackage with the file permissions on disk, so executables stay executable.

Identifier rewriting is keyed, not valued: only keys that hold bundle identifiers are rewritten — `CFBundleIdentifier` and anything ending in `BundleIdentifier`, at any depth. That moves every cross-reference bundles hold to each other (`WKCompanionAppBundleIdentifier`, `WKAppBundleIdentifier`, `NSExtension` attributes) and nothing else. Replacing by value instead was tried and was wrong: a framework whose identifier is `VirtualStadiumDataSDK` carries that same word as `CFBundleExecutable`, the name of a file on disk, so the plist ended up pointing at an executable that did not exist and the build would not install. `NSExtensionPointIdentifier` is excluded for the same reason — it names one of Apple's extension points, not a bundle in this build. Matches are whole strings, so an identifier that merely begins with another is left alone. The bundle's own `CFBundleIdentifier` is then set outright, because a bundle signed for an identifier its Info.plist does not claim is one iOS refuses.

The signer never chooses entitlements. Each bundle is signed with the entitlements inside Apple's own provisioning profile for that identifier, read from the CMS payload. A capability the plan said would be lost is lost because Apple did not grant it, not because this code removed it.

Signing runs on a blocking thread: it reads and writes a whole app bundle and hashes every file. The plan is rebuilt from the IPA, team, and Watch choice rather than remembered, so the build that is signed is the build that was reviewed, and a plan whose profiles were never prepared is refused before the archive is opened. The signed build then goes through the same installation review as any other IPA — including whether the iPhone is in its profile — so the existing preflight validates the signer's own output.

A signing run records what each stage did — file and bundle counts, sizes, the identifiers already shown in the interface, each profile's expiry — and returns it with the result, where the interface shows it under "What signing did". The same lines go to the desktop log under `operation = "signing"`. Neither carries a path from the person's disk or a device identifier. A failure names the stage it happened in, because "it did not work" is not a diagnosis.

Signing needs a certificate in the current session, so the action is disabled until one is held and says so. The key survives a restart in the Keychain; the certificate is fetched from Apple again each session, and a certificate matching the stored key is reused without writing anything.

A free personal team has no certificates page at developer.apple.com. When its one slot is held by a certificate whose private key is not on this Mac — an earlier run before the key was stored, say — nothing can sign and nothing can be requested. Only then does the interface offer to withdraw the team's certificates, behind its own acknowledgement, stating that every app already signed with them stops launching. It is never offered otherwise and never happens on its own.

The rewritten identifier is itself a consequence, stated alongside the capability losses. Nothing in the build is wrong and no entitlement is involved: the identifier is the credential that identity providers and backends check, and it had to change for another team to sign at all. The plan names the SDKs it can see in the build whose services pin it — currently the Facebook SDK and Google Sign-In, by framework identifier — and says plainly that a company's own backend commonly pins it too. Because the suffix derives from the team, every tester's build has a different identifier, so each one has to be registered separately with whatever checks it.
