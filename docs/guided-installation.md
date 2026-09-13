# Guided installation

The production React workspace follows the reviewed prototype in `design/install-flow.html`.
App Library is the single persistent sidebar destination for apps. Its **Install an app** button opens the chooser. Signing or installation adds a temporary progress entry to the sidebar. Existing app detail and
`#/ipas/{app}/workspace/{artifact}` links enter the same workspace. The prototype's scenario
controls and sample account data are not part of the production application.

## Flow

Choose a library version or import originals → check the IPA and verified iPhone, on a cable or Wi-Fi → prepare
signing only when needed or requested → review the exact artifact → explicitly install → result.
Imports are sequential, retain partial failures, and never authorize signing or installation.

A profile problem may lead to signing assessment. Encryption, incompatible hardware/OS and
incomplete inspection block this route. Passing local checks does not establish signature trust
or launchability: iOS still validates the package.

Authentication and verification use the existing account service. Membership comes from Apple's
team data. A sole supported team is selected automatically; multiple teams require a choice.
Unknown membership and enterprise distribution cannot use guided signing. A valid authenticated
session skips login; no password or session persistence is added.

The preparation review lists device registration, development certificate creation, identifiers,
profiles, rewritten identifier and capability consequences. Each applicable consequence has an
explicit acknowledgement. Existing registrations and matching keys/certificates are reused;
cached profiles are reused only with known expiry and matching identifiers, device and certificate.
Watch profiles are conservatively fetched again. Certificate withdrawal is separate, available
only after a structured certificate-conflict failure, and requires its own destructive-action consent.

## Ownership and IPC

`useGuidedFlow` owns frontend transitions, stale-response generations, review disposal and
native-status reconnection. The workspace stays mounted while browsing App Library, Settings or Help.
The app shell disables account changes during operations. Required Watch decisions remain visible;
optional name markers live in advanced settings.

`SigningService` owns preparation tokens and progress. Its new commands are:

- `library_review_preparation`: inspect a managed original and list account resources without
  remote mutations. Returns a ten-minute token, required actions and signing consequences.
- `library_execute_preparation`: consume the token with explicit acknowledgements, reverify inputs
  and resource requirements, then prepare/sign/retain through the existing services.
- `library_discard_preparation`: discard only the matching unused token and release its lease.
- `library_preparation_status`: read typed progress, completed actions and source/output identities.

A plan stores the session generation, selected team, verified device identity, Watch choice,
normalized marker and artifact lease in Rust memory. Device identity and credentials never cross
into these IPC views or the library manifest. One account reservation spans the full preparation
sequence. Changed requirements require a fresh review; mutations are never automatically retried.

Installation reviews now include typed issues and `direct`, `needs_signing` or `blocked` readiness,
while retaining blocker messages for existing clients. Final execution retains the existing token,
byte fingerprint, verified device, cancellation and durable-history safeguards. The new
`certificate_conflict` failure code distinguishes certificate recovery from unrelated errors.

Preparation progress survives navigation and webview reconnection in the running process. A
process restart discards preparation authorization and never resumes remote changes automatically.
The existing installation journal continues to reconcile interrupted installations conservatively.
No library schema migration or new installation queue is introduced.

## Verification

The interface suite exercises direct/personal/paid flows, multiple and unknown teams, verification,
required consent, resource reuse, Watch choice, stale reviews, locked devices, missing originals,
navigation during operations, interrupted outcomes, drag/drop, library history, restart persistence,
icons, light/dark themes, keyboard focus, reduced motion and minimum desktop dimensions.

Rust tests cover typed readiness, consent, stale/consumed tokens, artifact leases, account reservation,
profile identity binding, and the existing signing/installation/storage regression suite.
Run the frontend build, Playwright suite, core Rust tests, workspace Clippy, formatting checks and
`cargo run --locked -p doc-audit` before delivery.

Physical verification still required: use a chosen original IPA and live Personal Team/Developer
Program accounts to exercise import → prepare/sign → explicit install → restart → reopen.
Discovery and verified pairing alone are not native end-to-end validation. Actual installation
requires its own artifact review and acknowledgement; development work does not bypass that gate.
