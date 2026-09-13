# Local authentication

Orbiter signs in to Apple using local macOS authentication support and direct Apple HTTPS requests. No public or loopback Anisette server is involved. Windows sign-in fails closed until a local Windows provider exists.

## Native adapter

An Objective-C bridge loads AOSKit and AuthKit from `/System/Library/PrivateFrameworks` only, resolves AOSUtilities and AKDevice, checks selectors and types, and requests OTP headers for DSID -2. From AKDevice it reads the Mac's device identifier, local-user identifier, device description, CFNetwork bundle version and Darwin release. The machine serial number is not collected.

These values stay in Rust and native memory and are used only in the Apple authentication flow. The bridge catches Objective-C exceptions, returns fixed status codes, and never prints material. Rust bounds the response to 64 KiB, wipes its buffer, and serialises access through a single-permit gate with a 15-second timeout.

A timed-out native call cannot be forcibly stopped. It keeps the permit, so repeated checks cannot spawn unlimited native calls; restart Orbiter if the framework stays blocked.

Selector reference: [SideStore/MacAnisette](https://github.com/SideStore/MacAnisette) (MIT, © 2025 nythepegasus), license kept in [licenses/MacAnisette-MIT.txt](licenses/MacAnisette-MIT.txt). Orbiter has its own bounded bridge. It does not redistribute Apple's frameworks, copy Sideloadly binaries, modify other apps, inject libraries, install Mail plug-ins, or weaken system protection. These private APIs may break in later macOS versions.

## Direct Apple requests

isideload 0.3.17 is vendored with a small patch: its AnisetteData fields are private and its account builder otherwise defaults to a remote provider. [Patch notes](../vendor/isideload/ORBITER-PATCHES.md) list every change; the MIT license is retained and no upstream cryptography is replaced.

The remote provider is removed and the builder requires an explicit local one. GrandSlam HTTP clients:

- bypass configured and system proxies
- refuse redirects
- require HTTPS on port 443, to apple.com or an Apple subdomain, with no URL credentials
- enforce a 30-second deadline
- **take a fresh connection per request** (see below)

Team listing uses the fixed `developerservices2.apple.com` endpoint. Launching Orbiter or inspecting an IPA makes no network request. OS-level traffic interception and system framework networking have not been independently audited.

## What is sent and kept

Your Apple account address, the authentication exchange, a 2FA code, and local authentication identifiers go to Apple.

| Item                        | Kept                                                        |
| --------------------------- | ----------------------------------------------------------- |
| Password, verification code | Never                                                       |
| Account token               | Process memory; expires 30 minutes after access, or at exit |
| Anisette state              | Not persisted                                               |
| Signing key                 | Keychain — see below                                        |

Sign-out clears Orbiter's session. It does not revoke Apple's server session or reset macOS's own authentication data. Temporary copies made by third-party libraries or the webview are not guaranteed to be zeroized.

If the retired remote preview was used, a `com.orbiter.desktop.authentication.v1` / `anisette_state` entry may remain in your credential store. Orbiter never reads it. Close Orbiter before removing that specific entry, and do not remove Apple's.

## Signing-key storage

The development signing key is the one secret Orbiter stores — in this Mac's login Keychain, as a generic password under service `com.orbiter.desktop.signing-key.v1`. The account name is a SHA-256 of the lowercased Apple address and the team identifier, so reading the Keychain does not disclose which account or team it belongs to, and accounts never share a key. The item is accessible only while the Mac is unlocked and explicitly not synchronisable, so it never reaches iCloud or another Mac. Storing again replaces it.

It is stored because the alternative was worse: a key held only in memory requested a new certificate on every restart, and a team allows only a few active ones — the limit was reached within two sessions in testing. With the key stored, a restart finds the certificate Apple already issued and writes nothing.

**Forget stored signing key** removes the item; removing an absent key is success. It does not revoke the certificate, which Orbiter says plainly, because Orbiter never revokes.

The bridge uses the Security framework directly rather than the `security` command, so the key never reaches a temporary file, a command line, or the process table, and is zeroized in Rust on the way in and out. An unsigned local build may prompt for Keychain access when its code identity changes between builds; that prompt is macOS asking on your behalf.

## Error reporting

Errors name the stage — initial login, password proof, trusted-device verification, or developer-token request — using allowlisted static labels. Raw upstream messages, URLs, response bodies and attachments are never shown.

- Apple code **-22320** is reported as unsupported federated sign-in.
- A step the adapter cannot complete, with no PET token, is reported as an unsupported additional sign-in step. This is the expected outcome for an account federated to an external identity provider.
- SRP server-proof mismatch and unreadable account payloads are reported as themselves, not as a generic fallback.
- Inconclusive outcomes (429, 5xx, timeouts, connect failures) say explicitly that Apple did not report the password or 2FA as wrong.
- An HTTP 429 body is classified into an allowlisted label — GrandSlam plist (with its numeric `ec`), JSON, HTML/XML, empty, or unrecognised — which distinguishes account throttling from an edge block. No server text survives classification.

Orbiter honours Apple's `Retry-After` when sent, capped at one hour, and otherwise debounces 60 seconds. Restarting clears the hold, which lives in process memory.

## Root cause of the reported sign-in failures

**Apple's edge refuses any GrandSlam request that reuses a pooled keep-alive connection.** A credential-free burst probe found it: five consecutive `o=init` POSTs to `gsa.apple.com/grandslam/GsService2` with a synthetic address returned Apple error -20101 for the first and an HTTP 429 edge block for every one after, including after three- and ten-second gaps. A sign-in therefore always died on its second request — the password proof.

With `pool_max_idle_per_host(0)` the same five POSTs all reached Apple's authentication service. GrandSlam clients now set that.

This single defect explains every reported symptom: the HTTP 503 at initial login, "the authentication adapter could not complete this step", and every 429. Corrections made while investigating — normalised client info (one AuthKit segment, not two), single-use local anisette material, a live CFNetwork/Darwin User-Agent — are each independently correct but none is established as having fixed a live sign-in. Upstream's `Connection: close` on the proof request had the right intent one request too late and stays removed.

Regression test, credential-free:

```sh
cargo test --locked -p orbiter-core --lib consecutive_requests_are_not_refused_by_apple_edge -- --ignored
```

## Request budget

Isolating that defect took twelve synthetic GrandSlam POSTs in a few minutes, which tripped a volume limit: the next real sign-in was refused with an edge 429 on its _initial_ request. Apple's limit applies to the machine or network, not only to an account.

Ration investigation against live endpoints — one request per hypothesis, credential-free tests kept `--ignored`, never in a loop. A blocked initial request is the signature of this state, not of a client defect, and it clears with time.

## What is verified

Confirmed live on 2026-09-12 with a personal Apple account: local macOS support, direct Apple HTTPS, initial login, password proof, developer-session authorization, team listing, and explicit team selection.

Not verified:

- **The entire two-factor path** — trusted-device codes, SMS selection, resend, the prompt's one-use identifier. No challenge appeared, because the Mac and the iPhone are signed in to the same Apple ID and Apple already trusts this machine. Enabling 2FA did not change that. Its first real test needs a tester's account on a machine that account does not already trust.
- Session expiry, refresh, and sign-out against a live session.
- Windows support.
- **Federated accounts.** A company account delegated to an external identity provider cannot sign in; this adapter implements password sign-in only. Working in Xcode confirms developer access, not compatibility. A Microsoft OAuth token or a Sign in with Apple identity token must never be treated as a developer credential.

## Test

1. Run `npm run tauri dev`, review the local-authentication disclosure, and enter a designated test account in the app only. Local support resolves as part of signing in — there is no separate check.
2. Complete 2FA, confirm the returned teams, select one explicitly, refresh, and sign out.
3. Confirm a local-provider failure stops sign-in, and that offline access, blocked direct connections, redirects, or unsupported destinations fail rather than falling back to a proxy or remote provider.

Native support, without the desktop app:

```sh
cargo test --locked -p orbiter-core --lib local_anisette::tests::native_local_authentication_support -- --ignored
```

Do not enable sensitive upstream logging for these tests.
