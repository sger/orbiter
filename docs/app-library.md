# Persistent app library

Import IPAs, keep versions, and see what was installed where. Importing never installs anything.

Import with **Import IPA** or by dropping files on the library. Imports run one at a time; an invalid file does not stop the rest. Open an app for **Versions**, **Installations**, and **Devices**.

Apps group by main bundle identifier. Bytes decide identity, not version labels — the same file imported twice is one version; different bytes are a new version even under the same labels. Opening an original starts signing; opening a signed build goes to installation review.

## Storage

Rust owns `<application-data>/library/`:

| Path                     | Holds               |
| ------------------------ | ------------------- |
| `manifest.json`          | Records (schema 2)  |
| `artifacts/<sha256>.ipa` | Saved IPAs          |
| `icons/<sha256>.png`     | Extracted app icons |

An older manifest is upgraded on its next write. A newer one is refused rather than loaded with fields this build would strip.

Import copies, hashes and inspects outside the metadata lock, then publishes the artifact and manifest atomically. A failed manifest write removes the bytes that import just published. Original files are never edited.

macOS converts embedded CgBI icons with Apple's image converter, under a timeout. Asset-catalog-only icons are not decoded yet.

## Identity

Apps, artifacts and attempts have stable IDs. Originals and signed builds are separate records; a signed record keeps its source ID, rewritten bundle identifier, team tag, marker, Watch choice and known expiration.

A device is stored as a SHA-256 tag of the verified device identity and a random library-local salt, with its display name beside it. Raw UDIDs and account credentials are never in the manifest.

IPC takes artifact IDs; Rust resolves the managed path. Older path-based commands resolve or import first.

## History

An attempt is recorded before installation starts, against the exact reviewed hash and verified device. The backend saves stage changes whether or not the page is visible. Recovery needs a matching terminal journal entry — an unmatched interrupted attempt becomes unknown, never success. Failed, cancelled and unknown outcomes are kept.

Backend leases stop signing, provisioning and reviewed installation artifacts being removed mid-operation, including while their page is hidden. Opening another version invalidates provisioning and review.

## Expiration

Expiration on an original describes its imported profile; on a signed build, that build.

A countdown exists only for a successful installation with a known expiry. An import never counts down, and an unknown expiry is silence rather than a guess. `renewal.rs` decides the wording and the arithmetic, so every screen agrees; days round down. A build signed for another team shows no countdown.

Three states, escalating:

| State        | When                                  |
| ------------ | ------------------------------------- |
| Quiet line   | More than two days left               |
| Warning      | Two days or fewer                     |
| Announcement | Stops launching today, or already has |

The last two also appear once in the window header, wherever you are, linking to the app — suppressed on that app's own pages, and while an operation is running.

Within an app, the copy that still launches leads: a re-sign on one tester's phone does not revive another's. Every attempt stays listed against its own device.

**Sign and install again** opens the guided flow on the _original_ the expired build came from — a spent profile cannot be re-signed — with that build's Watch choice and name marker filled in, and names the phone it went to. Both stay editable; every acknowledgement is still required. It appears only while that original is still saved. If the connected iPhone is not the one that build went to, the screen says so; if it cannot be identified, it says nothing.

Nothing is scheduled, queued or refreshed in the background, and no credential is stored to allow it.

None of these records establish that an app is currently installed, trusted or working.

## Removal

Removing a version removes its original and signed variants but keeps attempts and metadata as tombstones. Removing one signed build leaves its original. Removing an app needs explicit confirmation. None of this uninstalls anything from a phone.

History is capped at 200 attempts per app and 2,000 overall; the oldest finished attempts go first, and one still in flight never does.

Removal records tombstones and pending cleanup atomically. A failed metadata write leaves files intact. Startup only retries cleanup a person already asked for. Shared content survives while another record references it.

Interrupted publication can leave unreferenced bytes. These are counted and named on the storage line, never pruned automatically; **Reclaim unreferenced files** removes them on request. Files the library did not name are never touched.

There is no cloud sync, scheduled renewal, install queue, or automatic deletion.

## Verification

Rust tests cover managed-copy preservation, deduplication, same-label versions, damaged artifacts, failed writes, corrupt manifests, leases, deletion recovery, device tagging, attempt attribution, job recovery, expiry counting and rounding, the two-day warning threshold, foreign-team silence, resolving an expired build back to a signable original, schema upgrade and refusal, icon storage, bounded history, unreferenced bytes, and import rollback.

Browser tests cover the library screens, partial import failure, workspace navigation, installation review, removal confirmation, the countdown escalating and being announced once, refreshing an expired build with its device identified, misidentified or unknown, restart persistence, and light/dark layouts at 780 × 600.

Both use synthetic IPAs and synthetic IPC. Neither replaces a real phone. Before claiming native end-to-end validation:

1. Import a real IPA; confirm the source file is unchanged.
2. Open, authenticate, prepare and sign. Confirm a signed artifact appears under the original.
3. Review it for an unlocked USB iPhone, acknowledge, install. Confirm artifact, device and outcome.
4. Restart Orbiter. Confirm metadata, history, device tag and expiration persist.
5. Review a second installation independently — a prior install authorizes nothing.
