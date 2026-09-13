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

Findings distinguish `not_verified`, `requires_configuration`, `unsupported`, and `preserved`. Inspection alone emits the first three; `preserved` appears only once a team has actually prepared identifiers and profiles, because only then is anything established rather than guessed. Encryption is an unsupported input for re-signing; expired profiles need configuration. Push, domains, merchants, groups, keychain, Watch identity, and unknown entitlements require review. There is no silent entitlement stripping, identifier rewriting, or Watch/extension removal.

The plan does this: it inspects every nested code object, rewrites identifiers, states each capability it cannot carry with the consequence, and blocks rather than proceeding when an input cannot be signed. Entitlements are never chosen by Orbiter — each bundle is signed with the entitlements inside Apple's own profile for its identifier.

## Subsequent boundaries

Add device transport through evaluated `idevice` APIs before authentication. The authentication/team preview now uses in-memory sessions and the local macOS Anisette flow described below. Certificate and provisioning operations, bundle transformation and signing, and installation jobs all live in the Rust core (`certificates.rs`, `provisioning.rs`, `signer.rs`, `installation/`). SQLite can persist refresh/job metadata once resumable jobs exist. Signing outputs must use separate paths and preserve the original IPA; never automatically revoke certificates.

## Device discovery (Phase 2, first increment)

`devices.rs` uses exact-pinned idevice 0.1.65 with `usbmuxd` and `ring`; the installation module additionally enables `afc` and `installation_proxy`. Pairing mutation is not enabled. It lists transports through the local daemon, reads selected Lockdown metadata keys, obtains an existing pairing record into memory, and verifies a TLS session. Known non-iPhones are excluded; devices with unavailable metadata remain visible as unverified. Only an ephemeral mux ID, display metadata, connection category, and redacted status leave the core. No UDID, IP address, HostID, or key is exposed in reports/logs.

A phone reachable on more than one transport is one phone: the daemon lists it once per connection with one identity, and discovery collapses those into a single entry on its preferred transport, naming the other as `alternate`. A cable always wins — it is faster and does not stop working when someone walks out of range. The identity used to group them is dropped before the report is built, so no UDID or address is added to what leaves Rust.

Discovery has a three-second daemon deadline and three seconds per device. Up to thirty-two transports are read from the daemon and up to sixteen phones are probed, so the probe budget is spent per phone rather than per cable. The UI skips overlapping polls, refreshes every five seconds when idle, and clears selections that disappear. Stale sessions are not reused. A timeout/disconnection produces an unavailable state. Missing/unreadable pairing records are reported as unverified, rather than claiming that trust was rejected. A successful TLS session establishes pairing; lock state and Developer Mode remain unverified unless the device explicitly returns a locked error. `PasswordProtected` is not used to infer lock state.

The desktop tracing filter admits only Orbiter's structured events, excluding dependency logs that could contain pairing or protocol payloads. The standalone discovery CLI installs no tracing subscriber. Pairing records remain managed by the OS Apple service; Orbiter neither stores nor mutates them. Windows uses loopback only and requires its own hardware validation. Installation transport is implemented as the preview described below; its happy path was confirmed by the user; interruption/recovery validation is pending.

### Wi-Fi, and why pairing still needs a cable

An operation locates a phone by its identity, not by the number the daemon gave one of its
connections. That number belongs to a connection and changes when a phone moves between a cable and
Wi-Fi; the phone does not. So the number is used only for first contact — it is all the window has
for a phone nobody has identified yet — and every later lookup, including staging cleanup, resolves
by identity. A review therefore survives the cable being pulled, and cleanup no longer fails for the
sole reason that the phone moved. The identity check that used to compare UDIDs after resolving is
now structural: a mismatch cannot arise from a lookup keyed on identity.

A changed connection is reported, not refused. Moving to Wi-Fi changes how long a transfer takes,
and someone watching a progress bar slow down deserves to know it is the connection rather than the
phone.

Nothing about pairing changes, and nothing can: Orbiter reads an existing pairing record and never
creates, resets, or stores one. Apple shows the "Trust This Computer?" prompt over a cable only, and
the system does not advertise an unpaired phone over Wi-Fi at all — so a phone is either already
paired or invisible, and there is no first contact over Wi-Fi to support. Orbiter's part is to say
so when the list is empty rather than to work around it.

Wi-Fi also changes nothing about where requests go. A network install is still relayed by *this
Mac's own* device daemon, so `address()` and the test that pins it to the local socket are
untouched and still mean what they say.

## Existing-signature installation

App icons are read from the archive in process. An Apple-optimised (CgBI) PNG cannot be decoded here, so on macOS `app_icon` writes that one image to a temporary directory and asks the system image converter to re-encode it, with bounded dimensions, a bounded output and a three-second timeout — the only subprocess inspection starts, and the only file it writes. An icon that cannot be read is absent; it never fails the inspection.

`installation::prepare` binds an opaque review token to a private local IPA snapshot and the selected phone's UDID, held only in Rust memory. A SHA-256 fingerprint identifies the reviewed bytes. The review expires in ten minutes. Only one prepared review and one review/install operation are admitted per desktop process. The UI pauses discovery/selection while preparing or executing; the backend independently validates the token and acknowledgement and consumes the review once.

Preflight rejects missing/expired profiles, incomplete/encrypted executable inspection, unverified iPhone platform/architecture/OS metadata, and profiles that do not authorize the selected phone (unless `ProvisionsAllDevices` explicitly authorizes all devices). Nested iPhone bundles receive membership checks; Watch bundles stay included, with Watch-device authorization explicitly unverified. No identifier, entitlement, profile, or signature is modified. Installed-app lookup is limited to the main bundle ID. The review shows potential replacement, and execution repeats the lookup and rejects a changed installed version/build.

Before staging, execution checks the same physical UDID, pairing, the snapshot's profile conditions, and installed-app state again. AFC transfers in 256 KiB chunks to a UUID-named file under `PublicStaging`; the accumulated hash and byte count must match the review before dispatch. Cancellation is atomic with the transition into installation. After that boundary there is no cancellation or rollback promise. The code uses idevice's InstallationProxy command and progress callback, not the convenience uploader (which reads the entire IPA and uses a shared staging filename).

A job journal is atomically replaced using a private temporary file and synced before device-install dispatch. It records only a random job ID, stage, byte counts, optional iOS-reported percentage, redacted message, and cleanup-needed flag. It records no credentials, UDID, pairing record, source path, or app binary. Preparing/transferring jobs become failed after restart; dispatching/installing jobs become unknown. No job resumes or retries automatically. A reported iOS completion is the sole success signal; 100% progress alone is not success. Explicit device rejections are failed; transport loss after dispatch is unknown.

Individual AFC calls have a 15-second timeout over a cable and 60 seconds over Wi-Fi, where a write that would be instant can sit behind other traffic; device connection and lookup have a 25-second deadline covering several lockdown round-trips plus a TLS handshake; the overall job has a 45-minute backstop. The job deadline is one value rather than one per transport because the transport can change mid-job. `install_with_callback` has no deadline of its own and is watched for silence instead: three minutes without iOS reporting progress ends the wait. That interval is a judgement rather than a measurement — iOS reports every few seconds while working — and the outcome is recorded as **unknown**, never failed, because the install command reached the phone and may have completed. Normal terminal outcomes attempt to remove only the generated staging file after rechecking the physical device binding. Unknown outcomes leave staging alone because iOS may still be reading it. Cleanup failures are visible. Process-killed temporary snapshots and staging files are not automatically scavenged; the latest-job journal is not a complete refresh database. Atomic replacement is tested locally, but power-loss durability and multi-process coordination are not claimed. Run one desktop process at a time.

## Account authentication and team selection

`accounts::Accounts` owns one active attempt or in-memory session per process. Consent/input validation precedes work. Login has a ten-minute deadline; every 2FA prompt has an opaque one-use ID and masked phone labels. Sign-out invalidates the generation and aborts pending login. Late results cannot restore a signed-out account. Team selection is explicit and must belong to the current session. Refresh is serialized and limited to 60 seconds; failure clears account/team state. The local 30-minute expiry is checked on status and account operations. No credentials or tokens are persisted by Orbiter.

### Federated and Managed Apple IDs are out of scope, by choice

Orbiter signs in the way Xcode does for an ordinary Apple ID: it asks Apple for the material to prove it knows the password, and completes a password handshake (SRP) with it. An account federated to an organisation's identity provider — a Managed Apple ID from Apple Business Manager — has no Apple-side password at all. Apple accepts the request, answers without any of the password-verification fields, and reports no error of its own. Nothing has gone wrong; there was simply nothing to verify.

Xcode signs those accounts in by handing the login to a browser, letting the identity provider authenticate, and receiving tokens back. Orbiter does not, and will not: that exchange is Xcode's private arrangement with Apple rather than a documented interface, so it can only be reproduced by reverse engineering and can break whenever Apple changes it. Orbiter is an open-source tool that works outside Apple's own tooling, for a tester re-signing a build with **their own** Apple ID because a company team's device allowance is full. A corporate account is the opposite of the case it exists to serve, and supporting one would buy a fragile dependency for no gain — the company team being full is the premise.

So this is refused rather than half-supported. `isideload::auth_error_is_unsupported_account` identifies the three shapes that mean it (Apple's federation code `-22320`, an additional sign-in step the adapter does not implement, and the missing-password-fields answer above) and Orbiter says the account cannot be signed in and to use a personal Apple ID. It is kept distinct from both a rejected password and an inconclusive failure: a person must never be sent to reset a password that was never wrong.

The explicit LocalProvider resolves macOS authentication material before the account builder contacts Apple. A bounded Objective-C bridge links only Foundation and dynamically loads installed Apple frameworks. No authentication-support server or remote-provider fallback exists. The vendored upstream adapter enforces direct Apple HTTPS destinations, disables redirects/proxies/debug TLS, and exposes a constructor for local data. See [local-authentication.md](local-authentication.md) for API, privacy, timeout, licensing, and platform limits. Windows authentication returns unavailable. Existing-signature installation remains independent of account sign-in.


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

The device identifier is read through the existing verified-iPhone path in the installation module, which requires a readable pairing record, a verified session, and a real iPhone. It is handed straight to Apple with the device name, and never enters the account view, the report, the journal, or any log; the returned outcome carries only whether the device was already registered or was registered now, and how many devices the team then has. A team membership whose kind Apple did not establish is treated as the stricter free allowance rather than the permissive one.

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

## The seven days

A free team's profile lasts seven days and the installed app then refuses to launch, telling the tester nothing. Orbiter used to state this once, during signing, and never again.

The library is the memory of it. A countdown exists only for an attempt that reached `Installed` and whose expiry is actually known: a saved file is a fact about this Mac, and counting down from an import would be Orbiter claiming an installation it never performed. Unknown is not zero either — a record written before the library kept epoch seconds has no countdown rather than a fabricated one.

Expiry travels as seconds since the epoch from the moment it is read, alongside the display string, and both are taken from one chosen profile so the date shown and the date counted can never describe different bundles. `profile::Profile`, `provisioning::ProfileOutcome`, `signer::Signed`, `library::Artifact` and `library::Attempt` all carry the pair. Nothing parses a date back out of a display string.

`renewal.rs` owns the arithmetic and the wording. `standing_at` is integer division on twenty-four-hour boundaries, rounded down — six and a half days left is six, because a person planning around the number must never be told they have longer than they do. `line` is the only place the sentence exists, so the library page, the workspace banner and a screenshot of either cannot word the same fact differently, and `urgent` is the single rule for what a person must act on.

`Library::snapshot` reports every qualifying install, longest-lived first within each app. Longest-lived rather than most recent: the same app can be on two testers' phones, and a re-sign installed on one does not revive the copy on the other, so a screen leads with the copy that still launches while every attempt stays visible against its own device. `Library::expiry` answers for one artifact — the workspace is opened from one, so "is this about what I am looking at?" is settled by construction, and a signed build made from an original counts as that original's. Only the team can still differ, and a build signed for another team gets no countdown at all: a reassuring "5 days left" about somebody else's build is worse than silence.

`renewal.json` is no longer written. It is still read, and shown as the labelled legacy record it is, with the one control that can clear it.

Nothing here contacts Apple, re-signs, or schedules anything. The banner offers the same signing call the main control makes, and only when that control would accept it.

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

The signed build's main app can carry a short marker in front of its display name, on by default, so a tester who still has the company build installed can tell two identical icons apart. A prefix, because the Home Screen truncates the end of a name. `signer::marker` decides what is usable — trimmed, control characters dropped, bounded — so the rule holds wherever the value comes from rather than only where it is typed. Only `CFBundleDisplayName`, and only on the main app: `CFBundleName` is filename-adjacent, and rewriting one of those is what produced a build iOS refused to install; nested bundles have no icon a person sees. A bundle with no name is left without one rather than being given the marker as its name, and re-signing the same build every week does not stack markers.

## Device log capture

`diagnostics.rs` streams the iPhone's system log over the same local usbmuxd transport discovery and installation use, and it exists to answer one question: why did a screen in the re-signed build fail?

The iPhone's log is the whole device's — every app, every system service, and whatever personal detail those print. Orbiter does not want that and does not take it. A capture is started explicitly, keeps only lines mentioning the subjects it was given (the signed build's identifier and the app's name), counts and discards everything else, holds what it keeps in memory, and writes nothing to disk. It is refused outright unless it has been told which app it is about, so it cannot be used as a general device log reader. Each kept line is trimmed to 600 characters.

A capture stops when asked, after five minutes, or after 500 matching lines, whichever comes first, and reads with a short timeout so a quiet iPhone still honours cancellation. The desktop log records that a capture ran and how many lines matched — never their contents, which belong to the device. The panel appears only once a build has been signed, since there is nothing else it could be about.

The signed build and the build it was made from are normally installed side by side, and their processes share a name, so the filter is told both identifiers: a line naming the superseded one and not this one belongs to the other app and is dropped. The rewritten identifier contains the original as a substring, so this build's own lines name both and survive.

What the log cannot show: a failure inside a `WKWebView`. Web content errors, blocked requests, and JavaScript exceptions never reach the system log — the system log only records that the web process ran. A development-signed build carries `get-task-allow`, so Safari's Web Inspector can attach to it and show those directly; a company distribution build cannot be inspected that way, which makes the re-signed build the more diagnosable of the two.

## Interface structure

The frontend has IPAs, Settings, and Help routes within a shared app shell. It is organised by feature rather than by kind:

```
src/ipc/commands.ts      every call into the Rust core, typed, one function each
src/state/pipeline.ts    what is done, what is next, and why anything is refused
src/features/…           build, device, team, sign, install, diagnose, help
src/types.ts             the shapes crossing the IPC boundary
```

Components never name a command string. The whole IPC surface is one file, so what the interface can ask the backend to do is readable in one place, and a command's name or arguments change once.

`state/pipeline.ts` exists because of a real defect. Each control used to derive its own enabled state from whatever was in scope, and "Re-sign IPA" ended up clickable in a session holding no certificate: the button knew about prepared profiles and nothing else, so it offered an action the backend then refused with a sentence the screen never showed. Gates now come from `signBlocked` and `installBlocked` over one `Pipeline` value, and each returns the sentence explaining the refusal — so a disabled control and the reason beside it cannot disagree. The order of the checks is deliberate: the earliest unmet requirement is the one a person can act on.

The team panel reports one `TeamStatus` upward rather than several callbacks, which keeps the derivation above it and stops the same state being reconstructed in two places.

### Stages and help

Each step of the pipeline renders as a stage: a heading, its controls while it needs attention, and its result once it has one. A satisfied stage collapses to that result, because the column previously showed every control of every step at once and the one thing to do next was indistinguishable from the six already done.

Collapsing is not sequencing. Apple does not require a registered device before issuing a certificate, and identifiers can be registered before either; only the prerequisites that genuinely exist gate a step. A collapsed stage always offers **Change**, so a completed step is never a dead end — a team, a Watch choice or a device can all be revisited.

The sidebar provides app navigation: IPAs and Help. Signing progress is an always-visible timeline inside the IPA workspace; each labeled step scrolls to its controls. Contextual help buttons still open a slide-over, while sidebar Help opens a dedicated page. Help holds the prose the panels used to carry: what Orbiter does, what a free team cannot carry and why, the seven-day limit, the "Untrusted Developer" step, what is stored, and how to diagnose a failure — including that a web view's failures never reach the device log. Each stage links to its section, which is what allows the stages themselves to be a control and a result rather than three paragraphs.

### Styling

Tailwind v4 via the Vite plugin, with the palette the interface already had declared once in `@theme` in `src/styles.css`. Those tokens are the source of truth: the stylesheet's rules read them through `var(--color-…)` rather than repeating hex values, which is what had already gone wrong — the audit found three dead rule blocks and a dozen near-duplicate greens that had drifted apart.

Feature layouts can use utilities in the markup; shared controls and shell layout use scoped component classes. The remaining component rules stay in `styles.css` because they are genuinely shared across elements; they draw from the same tokens, so there is one palette and no second place for it to drift. Those rules are wrapped in `@layer components`, which is not cosmetic: unlayered CSS beats every layered rule regardless of specificity, so while they sat outside a layer a bare `button { border: 0 }` silently defeated a `border` utility on a button and the rail's progress dots lost their outline. Nothing reported a conflict, and specificity reasoning does not predict it.

One `Select` draws every dropdown. There were two kinds a few pixels apart — the device selector with its own bordered row, the team and Watch selects with the platform's native control — and a difference that size reads as meaning something it does not.

Browser tests run their own Vite server on port 1421, never the one `npm run tauri dev` occupies on 1420. Reusing that server meant tests silently exercised whatever bundle it had started with: a Vite config added afterwards was invisible to them, so a run could pass green against a build nobody was shipping. It cost real time to notice, and the fix is one port and `reuseExistingServer: false`.

App navigation and signing progress are separate. Progress is generated from the pipeline stages; adding a stage adds a shortcut inside the workspace. To add an app section, extend `AppRoute`, register its view in `App`, and add a typed `NavigationItem` to the sidebar. IPAs, Settings, and Help are exposed today.

### Frontend organization

The frontend continues to use React and TypeScript with Vite, Tailwind, and Tauri. `main.tsx` only mounts the app. `app` owns shell layout, hash navigation, and the expandable sidebar. Native links target `#/ipas`, `#/settings`, and `#/help`, support back/forward, and work with Tauri's asset protocol. Empty or unknown hashes normalize to IPAs. Sidebar expansion defaults at 1100px and a manual choice lasts until the app is remounted. Primary and utility navigation groups scroll independently as they grow.

`App` keeps the IPA workspace mounted but hidden while Help is selected. Inspection, account state, form values, and signing operations survive navigation without a global store or browser storage. Help reuses the same content as the contextual help panel. Route changes focus the destination heading. Signing progress shows numbered, connected steps horizontally, switching to a vertical timeline below 700px of content width. Each step remains a keyboard-accessible shortcut.

`components/ui` owns native form controls: `TextField`, `Checkbox`, and `Select`. Feature containers must not style their descendant input/select elements globally. Checkbox text has its own wrapping span, and select chrome is drawn once around a native select. Tokens live in `styles.css`; container queries adapt the workspace at 900px of content width and account fields at 440px.

`features/workspace` composes the inspection, account, compatibility, signing, and installation UI. Inspection and signing lifecycles live in feature hooks; `features/team/useAccounts.ts` owns account state, polling, and invalidation effects. Credentials remain in component memory. `state/pipeline.ts` remains the source of workflow gates, and `ipc/commands.ts` owns typed command names and payloads, including challenge IDs and the discriminated account answer. The routing layer uses browser hash events without a new dependency. No global state store or backend contract change is introduced.


### Appearance

Settings offers System (the default), Light, and Dark appearance. Only this preference is stored under `orbiter.appearance` in local storage; account data and credentials are unaffected. Startup applies the preference before React mounts. System mode follows live `prefers-color-scheme` changes, while explicit choices override the OS. Invalid or inaccessible storage falls back to System; changes still work for the session if storage cannot be written.

The neutral gray palette is defined by semantic CSS variables in `styles.css`. Root `data-theme` overrides supply dark values, and `color-scheme` adapts native controls. All component colors use tokens, including focus, controls, sidebar, contextual Help, and warning/error surfaces. Warning amber and error red retain their status meaning. Settings uses native radio controls and shares the app's existing hash navigation; the IPA workspace stays mounted during appearance changes.

Typography uses shared size tokens: 15px body text and controls, 14px labels and helper text, 13px compact metadata, 18px section headings, and 24px page titles. Timeline labels wrap within their steps; stage headings can wrap instead of crowding actions.

Shared spacing tokens define 12px heading/description gaps, 8px field/link gaps, and 16px action gaps. Password fields offer a non-submitting visibility control and return to masked mode when cleared or disabled. Empty compatibility content stays compact until inspection returns findings. Account action emphasis follows the earliest unfinished, enabled action without changing backend prerequisites or acknowledgements.

The UI uses the platform system font through one `--font-sans` token (San Francisco on macOS, Segoe UI on Windows), with no font downloads. Logs and code retain their monospace stack.

Motion uses 140ms hover/press transitions and 200ms content reveals. Route content fades without transforms so contextual Help retains its viewport positioning. Hover movement is limited to enabled actions and appearance choices on pointer devices. Reduced-motion preferences disable transitions, transforms, reveals, smooth scrolling, and spinner animation; textual operation status remains available.
