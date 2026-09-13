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

| Crate                 | Owns                                                                                               |
| --------------------- | -------------------------------------------------------------------------------------------------- |
| `crates/orbiter-core` | Inspection, plan, certificates, provisioning, signing, installation, library. No Tauri dependency. |
| `src-tauri`           | Window setup, IPC commands, one active inspection, cancellation, redacted structured logging.      |
| `src`                 | Rendering returned data and explicit unavailable states.                                           |

IPC permissions grant only a file dialog and drag/drop listeners. There is no shell, arbitrary filesystem, remote content, or updater permission. Custom commands expose inspection, discovery, review, and explicitly acknowledged installation.

Blocking archive reads run on Tokio's blocking pool. One atomic busy guard rejects overlapping jobs and a drop guard releases it on success, failure, or unwind. Cancellation is checked between stages, bundles, ZIP entries, and 64 KiB chunks; CMS, Mach-O and plist parsing is synchronous, so cancellation waits for that bounded parse. A late cancellation may lose to a completed inspection — no immediate-cancellation claim is made. Inspection has no persistent or resumable jobs; the installation journal is separate.

## Inspection boundary

Every archive entry is checked for unsafe paths and declared size, but only metadata, declared executables, profiles and candidate icons are decompressed. Resource contents and CRCs of unread entries are not verified. **No full signature verification or installability claim is possible from this report.**

| Input            | Limit                                                                                             |
| ---------------- | ------------------------------------------------------------------------------------------------- |
| IPA              | 2 GiB compressed; regular local file                                                              |
| ZIP directory    | 50,000 entries, 32 MiB, single volume; ZIP64, self-extracting and trailing-data variants rejected |
| Expanded archive | 8 GiB aggregate; 1 GiB per entry; above 1000:1 compression rejected                               |
| Data read        | 1 GiB cumulative; one executable at a time; 512 MiB each                                          |
| Bundles          | 512; exactly one direct `Payload/*.app`                                                           |
| Plists/profiles  | 4 MiB input; depth 64; 100,000 events; 8 MiB expanded scalars                                     |
| Mach-O           | 32 non-overlapping fat slices; bounded commands and signature slots                               |
| Icons            | 2 MiB standard PNG; no external URLs                                                              |

The `plist` streaming API is pinned exactly so depth and count limits apply before recursive `Value` construction. A ZIP end-directory preflight bounds allocation before the ZIP library is constructed, and central-directory counts are cross-checked because ZIP libraries may collapse duplicate names. Absolute, parent, backslash, colon, empty, control-character and trailing-space/dot components are rejected. Symlinked macOS-style frameworks are unsupported by this iOS inspector.

Recognised bundle roots are `.app`, `.appex` and `.framework` with direct Info.plists. Still future work: loose dylibs, resource bundles, other nested code layouts, DER entitlement decoding, cryptographic verification, and asset-catalog/CgBI icon decoding. Unknown or malformed executables and profiles produce unverified findings, and unknown capability keys are surfaced for review.

Per-slice entitlements and profile authorizations are separate concerns: an entitlement's presence is not evidence of authorization under a new team.

## Compatibility model

Findings are `not_verified`, `requires_configuration`, `unsupported`, or `preserved`. Inspection alone emits only the first three — `preserved` appears once a team has actually prepared identifiers and profiles, because only then is anything established rather than guessed.

Encryption is unsupported for re-signing; expired profiles need configuration. Push, domains, merchants, groups, keychain, Watch identity and unknown entitlements require review. There is no silent entitlement stripping, identifier rewriting, or Watch/extension removal.

The plan inspects every nested code object, rewrites identifiers, states each capability it cannot carry with its consequence, and blocks rather than proceeding when an input cannot be signed. Entitlements are never chosen by Orbiter — each bundle is signed with the entitlements inside Apple's own profile for its identifier.

## Device discovery

`devices.rs` uses exact-pinned idevice 0.1.65 with `usbmuxd` and `ring`; installation adds `afc` and `installation_proxy`. Pairing mutation is not enabled.

It lists transports through the local daemon, reads selected Lockdown keys, loads an existing pairing record into memory, and verifies a TLS session. Known non-iPhones are excluded; devices with unavailable metadata stay visible as unverified. Only an ephemeral mux ID, display metadata, connection category and redacted status leave the core — never a UDID, IP address, HostID or key.

Deadlines: three seconds for the daemon, three per device, capped at sixteen transports. The UI skips overlapping polls, refreshes every five seconds when idle, and clears selections that disappear. Stale sessions are never reused.

A timeout or disconnection is an unavailable state. A missing or unreadable pairing record is reported as unverified rather than as rejected trust. A successful TLS session establishes pairing; lock state and Developer Mode stay unverified unless the device returns an explicit locked error, and `PasswordProtected` is not used to infer lock state.

The desktop tracing filter admits only Orbiter's structured events, excluding dependency logs that could carry pairing or protocol payloads. The discovery CLI installs no subscriber. Pairing records stay managed by the OS; Orbiter neither stores nor mutates them. Windows uses loopback only and needs its own hardware validation.

## Installation

App icons are read in process. An Apple-optimised (CgBI) PNG cannot be decoded here, so on macOS `app_icon` writes that one image to a temporary directory and asks the system image converter to re-encode it, with bounded dimensions, bounded output and a three-second timeout — the only subprocess inspection starts, and the only file it writes. An unreadable icon is absent; it never fails inspection.

### Review

`installation::prepare` binds an opaque review token to a private IPA snapshot and the selected phone's UDID, both held only in Rust memory, with a SHA-256 fingerprint identifying the reviewed bytes. A review expires in ten minutes. One prepared review and one operation per process. The UI pauses discovery while preparing or executing; the backend independently validates the token and acknowledgement and consumes the review once.

Preflight rejects missing or expired profiles, incomplete or encrypted executable inspection, unverified iPhone platform/architecture/OS metadata, and profiles that do not authorize the selected phone (unless `ProvisionsAllDevices` authorizes all). Nested iPhone bundles get membership checks; Watch bundles stay included with Watch-device authorization explicitly unverified. Nothing is modified. Installed-app lookup covers the main bundle ID only; the review shows potential replacement and execution repeats the lookup, rejecting a changed version or build.

### Execution

Before staging, execution rechecks the same physical UDID, USB connection, pairing, profile conditions and installed-app state. AFC transfers in 256 KiB chunks to a UUID-named file under `PublicStaging`, and the accumulated hash and byte count must match the review before dispatch. Cancellation is atomic with the transition into installing; after that boundary there is no cancellation or rollback promise.

It uses idevice's InstallationProxy command and progress callback, not the convenience uploader, which reads the whole IPA and uses a shared staging filename.

| Timeout                 | Value  |
| ----------------------- | ------ |
| AFC call                | 15 s   |
| Device connect / lookup | 10 s   |
| Whole job               | 20 min |

### Journal and recovery

A journal is atomically replaced through a private temporary file and synced before dispatch. It records a random job ID, stage, byte counts, optional iOS percentage, redacted message and a cleanup flag — no credentials, UDID, pairing record, source path, or binary.

After a restart, preparing and transferring jobs become failed; dispatching and installing become unknown. Nothing resumes or retries automatically. A reported iOS completion is the sole success signal — 100% progress is not success. Explicit device rejections are failed; transport loss after dispatch is unknown.

Normal terminal outcomes remove only the generated staging file, after rechecking the device binding. Unknown outcomes leave staging alone, because iOS may still be reading it. Cleanup failures are visible. Snapshots and staging files from a killed process are not scavenged, and the journal is not a complete job database. Atomic replacement is tested; power-loss durability and multi-process coordination are not claimed. **Run one desktop process at a time.**

## Accounts and teams

`accounts::Accounts` owns one attempt or in-memory session per process. Consent and input validation precede work. Login has a ten-minute deadline, and every 2FA prompt carries an opaque one-use ID and masked phone labels.

Sign-out invalidates the generation and aborts a pending login, so a late result cannot restore a signed-out account. Team selection is explicit and must belong to the current session. Refresh is serialised and limited to 60 seconds; failure clears account and team state. The 30-minute local expiry is checked on every status and account operation. Orbiter persists no credentials or tokens.

An explicit LocalProvider resolves macOS authentication material before the account builder contacts Apple. See [local-authentication.md](local-authentication.md) for the bridge, privacy, timeouts, licensing and platform limits. Windows authentication returns unavailable, and existing-signature installation does not depend on sign-in.

### Federated and Managed Apple IDs are out of scope, by choice

Orbiter signs in the way Xcode does for an ordinary Apple ID: it asks Apple for the material to prove it knows the password, then completes an SRP handshake. An account federated to an organisation's identity provider — a Managed Apple ID — has no Apple-side password at all, so Apple accepts the request, answers without the password-verification fields, and reports no error. Nothing went wrong; there was nothing to verify.

Xcode signs those accounts in by handing the login to a browser and receiving tokens back. Orbiter will not: that exchange is Xcode's private arrangement with Apple, reproducible only by reverse engineering and breakable whenever Apple changes it. Orbiter exists for a tester re-signing a build with **their own** Apple ID because a company team's device allowance is full — a corporate account is the opposite of that case.

So it is refused rather than half-supported. `isideload::auth_error_is_unsupported_account` identifies the three shapes that mean it — Apple's federation code `-22320`, an unimplemented additional sign-in step, and the missing-password-fields answer — and Orbiter says to use a personal Apple ID. This stays distinct from a rejected password and from an inconclusive failure, so nobody is sent to reset a password that was never wrong.

## Re-signing plan

`plan.rs` computes what re-signing under another team would change. Pure local computation: it signs nothing, writes nothing, and contacts no Apple service.

Identifiers are rewritten deterministically from the target team ID, so the same team always produces the same identifiers and a weekly re-sign replaces the tester's app instead of installing a second copy. Nested bundles keep their relationship to the main app's new identifier; frameworks are rewritten but consume no App ID.

Each team-scoped entitlement becomes a decision carrying its action, reason and runtime consequence. Push, associated domains, Apple Pay merchants and app groups are proposed for removal under a Personal Team; keychain groups and application/team identifiers are rewritten; the development entitlement is set. Every decision is marked as needing portal confirmation, because Apple's published capability table does not cleanly separate Personal Team support — the portal's answer must override the proposal. Unrecognised `com.apple.developer.*` keys are proposed for removal rather than silently kept.

Blockers: encrypted executables, a missing main identifier, an unselected team, and exceeding a Personal Team's ten App IDs per seven days.

A Watch app is never removed silently and never kept silently. While the choice is open it is a blocker, because registering its identifier spends one of those ten and Watch provisioning under a Personal Team is unverified. `Remove` drops the Watch app and everything nested inside it and states that its watchOS features are gone; `Sign` keeps it and states that it may fail to install or run.

Read-only output today:

```sh
cargo run --locked -p orbiter-core --bin orbiter-sign-plan -- /path/to/company.ipa TEAMID --personal
```

It exits non-zero when the plan has blockers. Signing revalidates every decision against the real certificate, App ID and profile rather than trusting the plan.

## Device registration

`provisioning::register` is the first operation that writes to Apple. It refuses before any request unless a live session exists, a team is selected, and the consequence is acknowledged: a free team allows three devices, and a paid team consumes one of 100 slots for the membership year, which removing the device later does not return. The session object itself authorises the write, so a stale or forged view cannot reach Apple; tests check the refusal order.

The device identifier comes from the verified-iPhone path in the installation module — USB, a readable pairing record, a verified session, a real iPhone — and goes straight to Apple with the device name. It never enters the account view, report, journal or any log. The outcome carries only whether the device was already registered and how many the team now has.

A membership whose kind Apple did not establish is treated as the stricter free allowance. Registration lists the team's devices first, so an already-registered iPhone writes nothing. A timeout says explicitly that the request may still have been applied and to check developer.apple.com, because a portal write has no rollback.

## Development certificate

`certificates::ensure` obtains the certificate the signer uses. The RSA key is generated on this Mac and never leaves it — only a PKCS#10 request goes to Apple, and its subject carries no account, device or company information. A test asserts that and that the key is 2048-bit. Key generation runs off the async runtime.

The team's certificates are listed and matched against the key they certify, not their name, so a rerun with the same session reuses the existing certificate and writes nothing. Reaching Apple's active-certificate maximum is reported with the count and left to the person: **Orbiter never revokes**, because revoking invalidates every app already signed with that certificate, including apps Orbiter did not produce. A timeout says the certificate may still have been issued.

The key is stored in the Keychain (see [local-authentication.md](local-authentication.md)); the certificate is fetched from Apple again each session and reused without writing. Apple labels it with the computer name, bounded to plain text with a neutral fallback.

## App identifiers and profiles

`provisioning::ensure_app_id` registers the plan's rewritten identifiers, reusing any the team already holds so a rerun writes nothing. Apple's remaining-identifier count is reported, and a team with none left is refused with what to do rather than attempting a write. Registration is acknowledged first: a free team may register ten identifiers per seven days, and an identifier cannot be reused by another team afterwards.

This is where Apple answers the capability questions the plan could only propose. The created App ID reports which features Apple actually enabled; feature keys whose meaning is established by use are named, anything else is reported as an unnamed capability rather than guessed. A capability Apple reports as disabled is never presented as present.

`provisioning::fetch_profile` downloads the team provisioning profile for each identifier. Profile bytes stay in the session for the signer and are deliberately not serialised into the interface — only identifier, expiry and UUID are shown.

Provisioning runs the plan first and writes nothing when the plan has blockers. Changing the IPA, device or team clears the result rather than carrying it across.

## The seven days

A free team's profile lasts seven days, after which the installed app refuses to launch and tells the tester nothing.

Expiry travels as epoch seconds from the moment it is read, alongside the display string, both taken from one chosen profile — so the date shown and the date counted can never describe different bundles. `profile::Profile`, `provisioning::ProfileOutcome`, `signer::Signed`, `library::Artifact` and `library::Attempt` all carry the pair. Nothing parses a date back out of a display string.

`renewal.rs` owns the arithmetic and the wording:

- `standing_at` divides on twenty-four-hour boundaries, rounded down — six and a half days left is six, because nobody may be told they have longer than they do.
- `line` is the only place the sentence exists, so every screen and any screenshot agree.
- `urgent` and `due_soon` are the single rules for what must be acted on, and when a warning starts.

A countdown exists only for an attempt that reached `Installed` with a known expiry: a saved file is a fact about this Mac, and counting down from an import would claim an installation Orbiter never performed. Unknown is not zero.

`Library::snapshot` reports every qualifying install, longest-lived first within each app — longest-lived rather than most recent, because a re-sign on one tester's phone does not revive the copy on another's. `Library::expiry` answers for one artifact, and a signed build made from an original counts as that original's. A build signed for another team gets no countdown: a reassuring "5 days left" about somebody else's build is worse than silence.

`renewal.json` is no longer written. It is still read and shown as the labelled legacy record it is, with the one control that clears it.

Nothing here contacts Apple, re-signs or schedules anything. See [app-library.md](app-library.md) for the countdown states and the refresh action.

## The signer

`signer.rs` turns a reviewed plan, Apple's profiles and this Mac's certificate into a new IPA. The selected IPA is opened read-only and never written; work happens in a temporary directory inside the application's own storage, removed when the operation returns, and the result is a separate file named for the team.

Order of work:

1. Extract under the same refusals inspection applies — unsafe paths, symlinks, special files, case-colliding duplicates, 2 GiB / 50,000 entries / 8 GiB expanded.
2. Remove every bundle the plan left out, shallowest first, so a removed Watch app takes its extensions and frameworks with it and is reported once.
3. Rewrite identifiers.
4. Install one profile per bundle that holds an App ID.
5. Sign from the inside out.
6. Repackage with the file permissions on disk, so executables stay executable.

Identifier rewriting is **keyed, not valued**: only keys holding bundle identifiers are rewritten — `CFBundleIdentifier` and anything ending in `BundleIdentifier`, at any depth. That moves every cross-reference between bundles (`WKCompanionAppBundleIdentifier`, `WKAppBundleIdentifier`, `NSExtension` attributes) and nothing else. Replacing by value was tried and produced builds iOS refused to install, because an identifier can also be the name of a file on disk. `NSExtensionPointIdentifier` is excluded for the same reason — it names one of Apple's extension points, not a bundle. Matches are whole strings. The bundle's own `CFBundleIdentifier` is then set outright, since iOS refuses a bundle signed for an identifier its Info.plist does not claim.

The signer never chooses entitlements: each bundle is signed with the entitlements inside Apple's profile for that identifier, read from the CMS payload. A capability the plan said would be lost is lost because Apple did not grant it.

Signing runs on a blocking thread. The plan is rebuilt from the IPA, team and Watch choice rather than remembered, so the build signed is the build reviewed, and a plan whose profiles were never prepared is refused before the archive is opened. The signed build then goes through the same installation review as any other IPA, so the existing preflight validates the signer's own output.

Each run records what every stage did — file and bundle counts, sizes, identifiers already shown, each profile's expiry — returned with the result and shown under "What signing did", with the same lines under `operation = "signing"` in the desktop log. Neither carries a disk path or device identifier. A failure names the stage it happened in.

A free team has no certificates page at developer.apple.com. When its one slot is held by a certificate whose private key is not on this Mac, nothing can sign and nothing can be requested. Only then does the interface offer to withdraw the team's certificates, behind its own acknowledgement, stating that every app already signed with them stops launching.

### Two stated consequences

**The rewritten identifier.** Nothing is wrong with the build and no entitlement is involved — the identifier is the credential identity providers and backends check, and it had to change for another team to sign at all. The plan names SDKs it can see whose services pin it (currently the Facebook SDK and Google Sign-In, by framework identifier) and says plainly that a company backend commonly pins it too. Because the suffix derives from the team, every tester's build has a different identifier.

**The name marker.** The signed main app can carry a short marker before its display name, on by default, so a tester who still has the company build installed can tell two identical icons apart. A prefix, because the Home Screen truncates the end. `signer::marker` decides what is usable — trimmed, control characters dropped, bounded — so the rule holds wherever the value comes from. Only `CFBundleDisplayName`, and only on the main app: `CFBundleName` is filename-adjacent, and rewriting it produced a build iOS refused. A bundle with no name is left without one, and re-signing weekly does not stack markers.

## Device log capture

`diagnostics.rs` streams the iPhone's system log over the same usbmuxd transport, to answer one question: why did a screen in the re-signed build fail?

That log is the whole device's — every app, every system service, and whatever personal detail those print. Orbiter does not take it. A capture is started explicitly, keeps only lines mentioning the subjects it was given (the signed build's identifier and the app's name), counts and discards everything else, holds what it keeps in memory, and writes nothing to disk. It is refused outright unless told which app it is about, so it cannot be a general device log reader. Kept lines are trimmed to 600 characters.

A capture stops when asked, after five minutes, or after 500 matching lines, and reads with a short timeout so a quiet iPhone still honours cancellation. The desktop log records that a capture ran and how many lines matched, never their contents. The panel appears only once a build has been signed.

The signed build and its source are normally installed side by side and their processes share a name, so the filter is given both identifiers: a line naming the superseded one and not this one is dropped. The rewritten identifier contains the original as a substring, so this build's own lines survive.

**What it cannot show:** a failure inside a `WKWebView`. Web content errors, blocked requests and JavaScript exceptions never reach the system log. A development-signed build carries `get-task-allow`, so Safari's Web Inspector can attach and show them directly — which makes the re-signed build the more diagnosable of the two.

## Interface

React, TypeScript, Vite, Tailwind and Tauri. Organised by feature:

```
src/ipc/commands.ts      every call into the Rust core, typed, one function each
src/state/pipeline.ts    what is done, what is next, and why anything is refused
src/features/…           build, device, team, sign, install, renew, diagnose, help
src/types.ts             the shapes crossing the IPC boundary
```

Components never name a command string. The whole IPC surface is one file, so what the interface can ask of the backend is readable in one place.

`state/pipeline.ts` exists because of a real defect: each control used to derive its own enabled state, and "Re-sign IPA" became clickable in a session holding no certificate — the button knew about prepared profiles and nothing else. Gates now come from `signBlocked` and `installBlocked` over one `Pipeline` value, each returning the sentence explaining the refusal, so a disabled control and the reason beside it cannot disagree. Check order is deliberate: the earliest unmet requirement is the one a person can act on.

The team panel reports one `TeamStatus` upward rather than several callbacks, which keeps derivation above it.

### Stages and navigation

Each pipeline step renders as a stage: a heading, its controls while it needs attention, its result once it has one. A satisfied stage collapses to that result.

Collapsing is not sequencing. Apple does not require a registered device before issuing a certificate, and identifiers can be registered before either — only genuine prerequisites gate a step. A collapsed stage always offers **Change**, so a team, Watch choice or device can be revisited.

The sidebar holds app navigation (IPAs, Settings, Help); signing progress is a separate always-visible timeline inside the workspace, each labelled step scrolling to its controls, horizontal above 700px of content width and vertical below. Contextual help opens a slide-over; sidebar Help opens a page carrying the prose the panels used to: what Orbiter does, what a free team cannot carry, the seven-day limit, the "Untrusted Developer" step, what is stored, and how to diagnose a failure.

Routes are hash-based with no new dependency: `#/ipas`, `#/settings`, `#/help`, supporting back/forward and Tauri's asset protocol. Empty or unknown hashes normalise to IPAs. Route changes focus the destination heading. `App` keeps the IPA workspace mounted but hidden while Help or Settings is selected, so inspection, account state, form values and signing survive navigation without a global store or browser storage.

Sidebar expansion defaults at 1100px; a manual choice lasts until remount. To add a section, extend `AppRoute`, register the view in `App`, and add a typed `NavigationItem`.

### Styling

Tailwind v4 via the Vite plugin, with the palette declared once in `@theme` in `src/styles.css`. Those tokens are the source of truth and rules read them through `var(--color-…)` rather than repeating hex values — an audit had already found three dead rule blocks and a dozen near-duplicate greens that had drifted apart.

Feature layouts use utilities in markup; shared controls and shell layout use scoped component classes, which stay in `styles.css` because they are genuinely shared. They are wrapped in `@layer components`, which is not cosmetic: unlayered CSS beats every layered rule regardless of specificity, so while they sat outside a layer a bare `button { border: 0 }` silently defeated a `border` utility.

`components/ui` owns native form controls — `TextField`, `Checkbox`, `Select`. Feature containers must not style descendant inputs globally. One `Select` draws every dropdown; there used to be two kinds a few pixels apart, and a difference that size reads as meaning something it does not. Container queries adapt the workspace at 900px of content width and account fields at 440px.

### Appearance

Settings offers System (default), Light and Dark. Only this preference is stored, under `orbiter.appearance` in local storage; account data is unaffected. Startup applies it before React mounts. System mode follows live `prefers-color-scheme` changes. Invalid or inaccessible storage falls back to System, and changes still work for the session if storage cannot be written.

Root `data-theme` overrides supply dark values and `color-scheme` adapts native controls. All component colours use tokens, including focus, sidebar, help and warning/error surfaces. Warning amber and error red keep their status meaning.

### Browser tests

Tests run their own Vite server on port 1421, never the 1420 that `npm run tauri dev` occupies, with `reuseExistingServer: false`. Reusing that server meant tests silently exercised whatever bundle it had started with, so a run could pass green against a build nobody was shipping.
