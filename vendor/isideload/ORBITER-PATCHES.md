# Orbiter local-authentication patches

Based on published isideload 0.3.17 (MIT). The source tree was compared against the upstream repository during review on 2026-09-12. Upstream license is retained in LICENSE. This path is selected by the workspace crates.io patch; it is not a new published package.

Changes:

- Remove the remote_v3 provider module and its source files.
- Require an explicit provider in AppleAccountBuilder; never fall back to a remote provider.
- Add AnisetteData::from_local so the separate Orbiter OS adapter can construct authentication data without making existing private fields public.
- Disable HTTP redirects and configured/system proxies, require HTTPS, and use a 30-second network deadline for GrandSlam clients.
- Reject proxy/debug-TLS configuration.
- Validate selected destinations returned by Apple's URL bag and all request-builder URLs: HTTPS, apple.com or a subdomain, port 443, and no URL credentials.

Authentication cryptography and signing algorithms are unchanged. Orbiter uses only authentication, developer-session creation, and team-listing APIs. The upstream signing dependencies are still present and have their own licenses; see the dependency inventory. Unsupported Apple endpoints cause an error rather than weakening the restriction. Apply and test these patches during any dependency upgrade.

The directory may include unused metadata outside the allowed destination policy. It is retained as metadata; selected URLs are validated before any request. Orbiter also exposes a redacted error classifier that returns only allowlisted categories and numeric Apple/HTTP status codes, never raw server messages or attachments.

- Authentication diagnostics now attach allowlisted request-stage labels, distinguish explicit federation error -22320 from HTTP 503, and retain full response redaction.
- Service-directory fetch checks HTTP status before parsing.
- Trusted-device 2FA continues when optional SMS-number discovery fails; SMS discovery remains required for the SMS path.

- Correct GrandSlam client-data flag types from strings to plist booleans. Include local-user ID, local routing info (zero), current UTC timestamp, UTC timezone, and locale. Encode routing info as an integer in the plist and text in HTTP headers. Serial number is still excluded. Core regression coverage verifies a serialized plist round-trip using synthetic data.


September 12 initial-login 503 fix: the local provider now uses `com.apple.akd/1.0` instead of the legacy hardcoded `com.apple.dt.Xcode/25183.54.10` client token, matching [upstream isideload PR #11](https://github.com/nab138/isideload/pull/11), merged September 10 (commit a19f5f0dffac16123fd6f8e03a7598fd8ca43d43). Actual local machine/OS description, Apple-only HTTPS policy, and local anisette generation are retained. A credential-free A/B POST with synthetic headers and body `t` reproduced HTTP 503 with the old token and HTTP 404 with the replacement. The 404 is expected for that deliberately invalid body; it demonstrates different server handling, not a successful account login. A unit regression checks the production client-info constructor. Live account/2FA remains to be verified by the user.

- Classify Apple HTTP 429 as `SideloadError::RateLimited`, preserving Apple's `Retry-After`, before `error_for_status` discards the response. Add `auth_throttle_delay` and `auth_error_is_inconclusive` so callers can distinguish throttling, outages, and transport failures from a rejected credential.
- Report an unsupported additional sign-in step as `SideloadError::UnsupportedStep` instead of an untyped message, and label SRP server-proof mismatch and unreadable sign-in payloads with allowlisted contexts. Redaction still never emits Apple's own text.
- Treat locally generated anisette as single-use and give the GrandSlam proof request its own client-provided data, instead of replaying the initial request's one-time password and client time.
- Classify the body of a throttled (HTTP 429) response into an allowlisted shape label, including its numeric GrandSlam code, without retaining server text.
- Stop sending `Connection: close` on the proof login request, so it is not the only request forcing a fresh connection.
- Stop sending Xcode's `X-Xcode-Version` header, which contradicted the akd identity in `X-Mme-Client-Info` and `User-Agent`.
- Take a fresh connection per GrandSlam request (`pool_max_idle_per_host(0)`). Apple's edge answers HTTP 429 with a block page to any request reusing a pooled keep-alive connection, which refused every sign-in at its second request.
