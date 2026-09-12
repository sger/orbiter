# Local authentication — 2026-09-12

Orbiter uses local macOS authentication support and direct Apple HTTPS requests. No public or loopback Anisette server is configured. Windows sign-in fails closed until a local Windows provider is implemented.

## Native adapter

The Objective-C bridge loads AOSKit and AuthKit only from `/System/Library/PrivateFrameworks`, resolves AOSUtilities and AKDevice, checks selectors/types, and requests OTP headers for DSID -2. It obtains the Mac's device identifier, local-user identifier, and device description from AKDevice. It does not collect the machine serial number. These values stay in Rust/native memory and are used only in the Apple authentication flow. The bridge catches Objective-C exceptions, returns fixed status codes, and never prints material. Rust bounds the response to 64 KiB, wipes its owned buffer, and serializes access through a single-permit gate with a 15-second timeout. A timed-out native call cannot be forcibly stopped, but retains the permit so repeated checks cannot create unlimited native calls; restart the app if the framework stays blocked.

The selector/API reference is [SideStore/MacAnisette](https://github.com/SideStore/MacAnisette), MIT, copyright 2025 nythepegasus. Its license is preserved in [licenses/MacAnisette-MIT.txt](licenses/MacAnisette-MIT.txt). Orbiter has its own bounded bridge and does not redistribute Apple's frameworks, copy Sideloadly binaries, modify other apps, inject libraries, install Mail plug-ins, or weaken system protection. These private APIs may break in later macOS versions. A local generation test passed on this Apple Silicon Mac running macOS 26.5; this is not Apple-account authentication acceptance.

## Direct Apple requests

A small vendored patch of isideload 0.3.17 is required because its AnisetteData fields are private and its account builder otherwise supplies a remote provider by default. [Patch notes](../vendor/isideload/ORBITER-PATCHES.md) identify every change; the MIT license is retained. No upstream authentication cryptography is replaced.

The remote provider code is removed. The builder requires an explicit local provider. GrandSlam HTTP clients bypass configured/system proxies, refuse redirects, require HTTPS, and enforce a 30-second request deadline. Selected URL-bag endpoints and all authentication request-builder destinations must be apple.com or an Apple subdomain, port 443, without URL credentials. Team listing uses the fixed developerservices2.apple.com endpoint. The adapter makes no network request merely to launch Orbiter or inspect an IPA. Local framework checks are explicit; macOS manages any internal Apple-service interaction needed by its frameworks. OS-level traffic interception and system framework networking have not been independently audited.

Your Apple account email, authentication exchange, 2FA code, and local authentication identifiers are used with Apple. Passwords/codes are not persisted; account tokens stay in process memory and expire on access after 30 minutes or at process exit. Sign-out clears Orbiter's session and does not revoke Apple's server session or reset macOS's own authentication data. No new Anisette state is persisted by Orbiter. Temporary copies made by third-party libraries or the webview are not guaranteed to be zeroized.

If the retired remote preview was used, its `com.orbiter.desktop.authentication.v1` / `anisette_state` credential-store entry may remain. The local implementation never reads it. Close Orbiter before removing that specific obsolete entry from your OS credential manager; do not remove Apple's entries. No real credentials were supplied by the agent during implementation.

## Test

1. Start `npm run tauri dev`, review the local-authentication disclosure, and enter a designated test account in the application only. Local support is resolved as part of signing in.
3. Complete 2FA, confirm the returned teams, select one explicitly, refresh, and sign out.
4. Local-provider failure must stop sign-in. Offline Apple access, blocked corporate direct connections, redirects, or unsupported URL-bag destinations must fail rather than use a proxy or remote fallback.

The explicit native generation smoke test emits no authentication data:

```sh
cargo test --locked -p orbiter-core local_anisette::tests::native_local_authentication_support -- --ignored
```

Live Apple sign-in and Windows support remain unverified. Re-signing/provisioning are separate unfinished work.


## Setup regression fixes

A user sign-in failure exposed two setup defects before any account could be verified: missing explicit ring provider initialization for reqwest's rustls-no-provider mode, and validating every directory metadata value instead of the selected endpoint. Initialization now installs the TLS provider; selected destinations and actual request builders remain restricted to Apple HTTPS. The credential-free native Apple directory test now passes. This tests local support plus the real directory fetch, not account login or 2FA success.

Authentication errors now distinguish setup from account verification and expose only allowlisted categories or numeric Apple/HTTP codes. Raw upstream messages, URLs, response bodies, and attachments remain hidden. A regression test verifies redaction of synthetic secret-bearing errors.


## Authentication diagnosis and federation

The company account is reported to work in Xcode and to use Microsoft federation. This confirms developer access in Xcode, not compatibility with the current Orbiter password adapter. HTTP 503 does not identify federation or invalid credentials as the cause. Errors now identify the initial login, password proof, trusted-device verification, or developer-token request using allowlisted static labels only. Apple's explicit authentication code -22320 is presented as unsupported federated sign-in, without displaying the server response or redirect URL.

Trusted-device verification no longer requires successful SMS-number discovery. Directory HTTP errors are checked before attempting plist parsing. These changes improve diagnosis and one Apple 2FA path; they do not implement Microsoft federation. No browser callback/token exchange for a developer session has been verified, and the app must not treat a Microsoft OAuth token or a Sign in with Apple identity token as a developer credential. A fresh user-initiated native sign-in is still required to locate the reported 503 precisely. Do not enable sensitive upstream logging for this test.


## Initial-login request format

A subsequent native attempt located HTTP 503 at the initial GrandSlam request, before password proof or 2FA. The vendored adapter had serialized `bootstrap`, `icscrec`, `pbe`, and `prkgen` as strings and omitted local-user ID, routing information, client time, locale, and timezone. These now use plist booleans and local metadata; local routing info is integer zero in the plist and a string in HTTP headers. UTC is paired with a UTC timestamp. Device serial number remains excluded. This aligns those fields with the independently reviewed request structure in [AltSign authentication](https://github.com/rileytestut/AltSign/blob/master/AltSign/Apple%20API/ALTAppleAPI%2BAuthentication.m). No implementation was copied from that source.

A regression test round-trips the actual request dictionary through XML plist serialization and checks types and synthetic metadata. This is a request compatibility correction, not proof that it caused the observed 503 or that federation works. Live sign-in must be retried in the rebuilt app.


September 12 initial-login 503 fix: the local provider now uses `com.apple.akd/1.0` instead of the legacy hardcoded `com.apple.dt.Xcode/25183.54.10` client token, matching [upstream isideload PR #11](https://github.com/nab138/isideload/pull/11), merged September 10 (commit a19f5f0dffac16123fd6f8e03a7598fd8ca43d43). Actual local machine/OS description, Apple-only HTTPS policy, and local anisette generation are retained. A credential-free A/B POST with synthetic headers and body `t` reproduced HTTP 503 with the old token and HTTP 404 with the replacement. The 404 is expected for that deliberately invalid body; it demonstrates different server handling, not a successful account login. A unit regression checks the production client-info constructor. Live account/2FA remains to be verified by the user.

Sign-in UX: a separate local-support preflight is optional; account sign-in already resolves local support before its first Apple request. The button now explains unmet requirements beside it, including cleared passwords after an attempt. A manually detected support failure still requires a successful recheck. Consent, backend availability, and operation guards remain enforced.


## September 12 sign-in regression fixes

Two reported failures were investigated: a federated company account ending in "The authentication adapter could not complete this step", and a personal account ending in "Apple password verification: Apple returned HTTP 429".

**Malformed client identification.** A local probe showed `AKDevice serverFriendlyDescription` already ends with an AuthKit segment naming the calling process, for example `<Mac14,10> <macOS;26.5;25F71> <com.apple.AuthKit/1 (com.orbiter.desktop/0.1.0)>`. Orbiter appended a second `<com.apple.AuthKit/1 (com.apple.akd/1.0)>` segment, so every request carried an `X-Mme-Client-Info` header with two client segments, which no Apple client sends. The provider now keeps only the machine and OS description before appending the akd segment. A credential-free A/B of the initial GrandSlam request with a synthetic non-existent address returned Apple error -20101 for the duplicated, corrected, and unmodified descriptions alike, so this correction is not demonstrated to be the cause of either reported failure; it removes a real deviation from the client format. The credential-free directory and local-support tests pass with the corrected header.

**HTTP 429 is throttling, not a verdict.** `error_for_status` discarded Apple's `Retry-After`, and the failure was presented as "Apple account authentication or two-factor verification failed", which points a user at a password that Apple never checked. GrandSlam now classifies 429 before that point and carries Apple's `Retry-After`. Inconclusive outcomes (429, 5xx, timeouts, connect failures) are prefixed with a statement that Apple did not report the password or two-factor verification as wrong. Orbiter also holds further attempts locally until the wait lapses (Apple's `Retry-After`, otherwise 15 minutes, capped at one hour), because each further attempt extends Apple's block. That wait survives sign-out: it is Apple state, not Orbiter session state. This does not lift an existing block. Apple throttles after repeated failed sign-ins from a Mac or account, so the personal-account 429 is expected to clear on its own after the wait; the company-account attempts are the most likely origin.

**Unsupported additional sign-in steps are named.** When Apple asks for a step the adapter cannot complete and no PET token is present, the previous code produced an untyped error that fell through to the generic adapter message. That path is the expected outcome for an account whose sign-in is delegated to an organisation's identity provider. It is now a typed error reported as an unsupported additional sign-in step, without echoing Apple's text. Federation itself is still not implemented; the company account is expected to keep failing, now with an accurate reason. SRP server-proof mismatch and unreadable account payloads are likewise reported as such instead of the generic fallback.

Regression tests cover client-info normalisation for plain, bundled, and unnamed process descriptions; throttle classification, bounds, and redaction; the unsupported-step message; and the local wait blocking a new worker while surviving sign-out.


## Proof-request one-time password replay

A retry with the corrected client info reproduced HTTP 429, and the stage label placed it on the proof request while the initial GrandSlam request succeeded. Both requests had carried the same `X-Apple-I-MD` one-time password: `AnisetteDataGenerator` cached local material for 60 seconds, and `login_inner` built one `cpd` for both steps. Locally generated material is now single-use — `needs_refresh` is always true for it, since regenerating costs one native call — and the proof request builds its own client-provided data. This is a request-correctness fix consistent with the observation that Apple accepted the first use of an OTP and rejected the second; it is not proof that replay caused the 429, and Apple may also be throttling the account after the earlier failed attempts.

The local retry hold was also too strong: with no Retry-After from Apple it invented a 15-minute block that stopped the user from testing a rebuilt adapter, on a different account from the throttled one. Orbiter now honours Apple's Retry-After when Apple sends one (capped at one hour) and otherwise debounces for 60 seconds only. Restarting Orbiter clears the hold in any case, since it is process memory.


## Reading Apple's 429 instead of guessing

A third attempt, with single-use local material, reproduced HTTP 429 on the proof request while the initial request again succeeded, so one-time-password replay was not the cause. The body of a 429 was being discarded along with the response. GrandSlam now classifies it into an allowlisted label — a GrandSlam plist (with its numeric `ec`, for -20101 and -22320 by name), a JSON body, an HTML or XML body, no body, or unrecognized — and reports that label with the 429. Apple's own service answering with a GrandSlam plist points at account-level throttling of password verification; an HTML or empty body points at an edge block of this client. No server text, URL, or payload is retained. A unit test checks each shape and that server text never survives classification.

Remaining untested asymmetries between the accepted init request and the refused proof request: the proof request is the only one that sends `Connection: close`, and every request mixes an akd client identity (`X-Mme-Client-Info`, `User-Agent`) with Xcode-specific `X-Xcode-Version` and `X-Apple-App-Info` headers. Neither can be A/B tested credential-free, because a synthetic address fails at the init step before a proof request is sent.


## The proof request's `Connection: close`

The user confirmed the personal account returned HTTP 429 on its first attempt, before any repeated sign-ins. Volume throttling of that account is therefore ruled out: Apple refuses this particular request. Both GrandSlam requests go to the same URL with the same headers and the same client-provided data, so the asymmetry — init accepted, proof refused — narrowed to one difference. Upstream sent `Connection: close` on the proof request only. That hop-by-hop header forces a fresh connection for every password proof, which is the shape an edge rate-limiter penalises, and nothing in the exchange requires it. Both requests now go out identically.

This is a single-variable change, chosen because it is the only header-level difference between the accepted and refused requests; it is not confirmed. If 429 persists, the next lever is the mixed client identity — akd in `X-Mme-Client-Info` and `User-Agent` alongside Xcode-specific `X-Xcode-Version` and `X-Apple-App-Info` — and the 429 body label now reported with the error distinguishes an Apple auth-service answer from an edge block.


## Edge block, and an impossible client identity

With `Connection: close` removed, the proof request still returned HTTP 429, and the new body label identified it as an HTML or XML body that is not a GrandSlam plist, with no Retry-After. Apple's authentication service never answered: an edge filter refused the request. Combined with the user's confirmation that the personal account returned 429 on its first attempt, account-level throttling is ruled out; this client is being refused.

The client identity it presented could not belong to a real machine. The User-Agent was a hardcoded `akd/1.0 CFNetwork/808.1.4` — a 2016 CFNetwork build — sent from macOS 26.5, alongside an `X-Xcode-Version: 27.0` header while `X-Mme-Client-Info` and the User-Agent both claimed akd. The native bridge now also reports this Mac's live CFNetwork bundle version and Darwin release (`uname`), and the User-Agent is built from them, currently `akd/1.0 CFNetwork/3860.600.21 Darwin/25.5.0`. Version text is accepted only if short and composed of digits, dots, and letters, so a hostile value cannot reach a header. `X-Xcode-Version` is no longer sent, since this client does not claim to be Xcode. `X-Apple-App-Info` remains: it names the GrandSlam scope being requested, not the client.

This is the second single-variable lever and is likewise unconfirmed. The credential-free directory fetch and local-support checks pass with the corrected identity. If Apple still refuses the proof request, the remaining coherent option is the opposite identity — present fully as Xcode in client info, User-Agent, and `X-Xcode-Version` — which the earlier 503 investigation rejected at the initial request but which has not been retried since these corrections.


## Root cause: Apple's edge refuses reused connections

A credential-free burst probe located it. Five consecutive `o=init` POSTs to `https://gsa.apple.com/grandslam/GsService2`, with a synthetic non-existent address, returned Apple authentication error -20101 for the first and an HTTP 429 edge block page for every one after it, including after three- and ten-second gaps. Nothing about the password, the proof request, two-factor verification, or either account was involved: Apple's edge refuses any request that reuses a pooled keep-alive connection to GrandSlam. A sign-in therefore always died on its second request — which is the password proof.

With `pool_max_idle_per_host(0)`, so each request takes a fresh connection, the same five POSTs all reached Apple's authentication service and returned authentication errors (-20101, then -20209 for the repeatedly probed synthetic address). GrandSlam clients now set that. Upstream's `Connection: close` on the proof request had the same intent but was one request too late: it closed the connection after the request that was already refused. It stays removed.

This also explains the earlier diagnoses. The reported HTTP 503 at the initial request, the "authentication adapter could not complete this step", and every HTTP 429 are consistent with this single defect, so the client-info and one-time-password corrections cannot be credited with fixing a live sign-in, and the mixed akd/Xcode identity was not the cause. `X-Xcode-Version` is restored alongside `X-Apple-App-Info`, matching the reference implementations; the corrected client info, single-use local material, and live CFNetwork/Darwin User-Agent are kept because each is independently correct.

A kept credential-free regression test sends two consecutive GrandSlam requests and fails if either is refused with 429:

```sh
cargo test --locked -p orbiter-core --lib consecutive_requests_are_not_refused_by_apple_edge -- --ignored
```

Live account sign-in past the password proof, two-factor verification, and team listing remain unverified.


## Request budget

Isolating the connection-reuse defect took twelve synthetic GrandSlam POSTs from this machine within a few minutes. That tripped a volume limit: the next real sign-in attempt was refused with an edge HTTP 429 on its *initial* request, a request that had succeeded every time before. Apple's limit applies to the machine or network, not only to an account, and it refuses the first request of a fresh attempt once tripped.

Investigation against live Apple endpoints must therefore be rationed: prefer one request per hypothesis, keep the credential-free tests `--ignored`, and do not run them in a loop. A blocked initial request is the signature of this state rather than of a client defect, and it clears on its own with time.


## Live sign-in confirmed — 2026-09-12

With a fresh connection per GrandSlam request, the user completed a live sign-in in the desktop app with a personal Apple account: initial login, password proof, developer session, and team listing all succeeded, and the account's team was returned and selectable. This confirms the connection-reuse defect was the cause of the reported failures; no earlier correction in this document is established as having fixed a live sign-in on its own.

Verified in this run: local macOS authentication support, direct Apple HTTPS requests, password proof, developer-session authorization, team listing, and explicit team selection. No verification-code prompt appeared, so Apple completed this sign-in without a two-factor challenge and the entire two-factor path — trusted-device codes, SMS selection, resend, and the prompt's one-use identifier — remains unexercised against Apple. Session expiry, refresh, and sign-out against a live session are also unrecorded. The company account remains unusable: its sign-in is federated to an external identity provider, which this password adapter does not implement.


## Two-factor on a shared Apple ID, and signing back in

The user enabled two-factor authentication on the personal account and still saw no verification prompt: the Mac and the iPhone are signed in to that same Apple ID, so Apple already trusts this machine and does not challenge it. The two-factor path therefore remains unexercised against Apple, and its first real test will be a tester's account on a machine that account does not already trust. This is a property of the account and machine, not evidence that the prompt works.

Signing out then left the sign-in button disabled with no obvious cause: sign-out cleared the account address and the consent checkbox along with the password, so two of the three preconditions silently reset. Sign-out and cancel now clear only the secrets — password and verification code — and keep the address and the consent given in this session, both still visible and editable. The backend still requires consent on every attempt and still never persists anything. The blocker text also names the single missing field rather than covering email and password in one sentence. A browser test signs out and asserts the address and consent survive, the button stays disabled until a password is typed, and then becomes enabled.


## Signing-key storage

The development signing key is the one secret Orbiter stores. It is written to this Mac's login Keychain as a generic password under the service `com.orbiter.desktop.signing-key.v1`, with the account name derived as a SHA-256 of the lowercased Apple account address and the team identifier — so reading the Keychain does not disclose which Apple account or team it belongs to, and different accounts and teams never share a key. The item is marked accessible only while this Mac is unlocked and explicitly not synchronisable, so it does not reach iCloud or another Mac. Storing again replaces the item rather than accumulating duplicates.

Nothing else is stored: no password, verification code, account token, pairing record, or device identifier. **Forget stored signing key** removes the item; a removal of an absent key is success. Removing the key does not revoke the certificate Apple issued for it, which is said plainly, because Orbiter never revokes.

Persistence exists because the alternative was worse. A key held only in memory meant every restart requested a new certificate, and a team allows only a few active ones, so a user hit the limit within two sessions — which is what happened in testing. With the key stored, a restart finds the certificate Apple already issued and writes nothing.

The bridge uses the Security framework directly rather than the `security` command, so the key is never written to a temporary file and never appears in a command line or the process table. The key is held in Rust as zeroized memory on the way in and out. An unsigned local development build may prompt for Keychain access when its code identity changes between builds; that prompt is macOS asking on the user's behalf and is expected.


## The separate support check is gone

Sign-in already resolved local macOS authentication support before its first Apple request, and reports a support failure as the reason sign-in stopped, so the separate **Check local support** button asked for a step that proved nothing extra. It is removed along with its IPC command, which also narrows the interface Orbiter exposes. The credential-free native check remains available without the desktop app:

```sh
cargo test --locked -p orbiter-core --lib local_anisette::tests::native_local_authentication_support -- --ignored
```

The browser test that covered the button now asserts the same property where it actually matters: an unavailable local provider stops sign-in, says so, and offers no remote alternative.
