# Backend architecture

How Orbiter's Rust is arranged, who owns which mutable state, and what the concurrency,
cancellation, persistence and recovery rules are.

This document is written for someone about to change the backend. It describes the intended shape
and the reasoning behind it; where the code has not caught up yet, the audit below says so.

---

## Phase 1 — audit of the starting point

Taken at `a06ba06`, before any of this refactor. ~8,300 lines of Orbiter-owned Rust across 27
files, 406 functions.

### Baseline checks

All green before any change, so every later failure is this work's:

| Check | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --offline --workspace --all-targets -- -D warnings` | clean |
| `RUSTDOCFLAGS="-D warnings" cargo doc -p orbiter-core --no-deps --document-private-items` | clean |
| `cargo test --offline --workspace` | 124 passed, 4 ignored |
| `cargo test --doc` | 0 doctests |
| `cargo run -p doc-audit` | **363 undocumented items in 27 files** |
| `npm run build`, `npx playwright test` | clean, 56 passed |

The four ignored tests are environment-dependent, not broken: `keychain::a_stored_key_survives_and_can_be_forgotten`
needs a real macOS Keychain, and three `local_anisette` tests reach Apple or the system's own
authentication frameworks. They are correctly ignored rather than faked.

### Where workflow coordination lives today

Not in the core. Three workflows exist only inside Tauri commands:

- **`execute_install`** (`src-tauri/src/main.rs`) validates the review token and staleness, takes
  the operation gate, consumes the prepared plan, registers a cancellation handle, records the
  library attempt, spawns the worker, filters status transitions, and performs crash recovery. A
  CLI or a test cannot run an installation; only a Tauri command can.
- **`library_sign`** (`src-tauri/src/library_commands.rs`) pins the source artifact, refuses signed
  inputs, computes the output directory, cleans the marker, signs, retains the result, and deletes
  the staging file. The lease that protects the source lives in a local binding.
- **`library_prepare_install`** binds a review to an artifact, compares hashes, and stores the
  plan plus its lease in Tauri-managed state.

`prepare_install` and `account_sign_ipa` are path-based compatibility shims that import first and
then delegate, so they are not a second implementation — that part is already right.

### Ownership of mutable state

| State | Owner today | Problem |
|---|---|---|
| `library::STORE` | **process-global `static Mutex<()>`** | Not owned by anything. Two `Library` values for different roots share one lock; tests in the same binary serialise on it. |
| `library::PINS` | **process-global `static Mutex<BTreeMap<PathBuf, usize>>`** | Same. A lease is keyed by path, so it is global by construction. |
| `Installations { gate, plan, control, current }` | Tauri `State` | Only reachable from a command. |
| `Accounts(Arc<Mutex<Inner>>, Arc<tokio::Mutex<()>>)` | Tauri `State` | Session, teams, certificate identity and provisioning results in one lock. |
| `Inspection`, `LogCapture` | Tauri `State` | Fine, but constructed implicitly by `.manage(Default::default())`. |

There is no application runtime. `Library::new(app_data_dir()/library)` is constructed fresh on
every command call; only the two statics make separate instances agree.

### Where blocking work runs

Correct today: `library_*` commands and `Accounts::sign_ipa` use `spawn_blocking` for copying,
hashing, inspection and signing. `installation::execute` is genuinely async (device transport).

The remaining hazard is lock scope rather than thread choice — `Library::read()` parses the whole
manifest and runs an O(artifacts × attempts) integrity pass on every public call, under a global
blocking mutex.

### Where runtime and persisted state can disagree

- `Installations.current` is a copy of the last `JobStatus`; the durable copy is
  `last-install.json`. A crash between them is reconciled by `job::recover` at startup, which
  downgrades a non-terminal stage to `Unknown` rather than inferring success. That rule is right
  and must survive this refactor.
- `Library::update` only writes on a stage change, so transient byte progress is deliberately not
  durable.
- A prepared review lives only in memory; its lease is dropped with the plan.

### Errors

`Result<T, String>` throughout: 13 sites in `accounts.rs`, 12 in `signer.rs`, 11 in
`library_commands.rs`, 7 in `installation/mod.rs`, and so on. The only structured error is
`orbiter_core::Error`, and it covers inspection alone.

Consequences: the frontend distinguishes failures by matching human-readable text; a caller cannot
tell "artifact missing" from "another operation is running" without reading prose; and the
messages double as both the user-facing explanation and the internal diagnostic, which is why they
are carefully worded but carry no machine-readable code.

### Identifiers

Every identifier is a `String`: app ids, artifact ids, attempt/job ids, review tokens, device tags,
team ids, bundle identifiers. `Library::expiry(artifact_id)`, `Library::remove(app_id, artifact_id)`
and `Library::begin(.., job_id, udid, ..)` all take bare strings, so transposing two arguments
compiles. The distinction that matters most — the ephemeral usbmuxd device id (`u32`) versus the
stable salted device tag (64 hex chars) versus the raw UDID that must never be stored — is carried
only by argument position and discipline.

### Existing public interfaces and storage

- Tauri commands: 27 registered in `generate_handler!`.
- CLI binaries: `orbiter-devices`, `orbiter-install-review`, `orbiter-sign-plan`. The last two
  duplicate small parts of workflows that will move into services.
- Storage: `library/manifest.json` (schema 2), `library/artifacts/<sha256>.ipa`,
  `library/icons/<sha256>.png`, `last-install.json`, and the legacy read-only `renewal.json`.

### Documentation and idiom gaps

363 items undocumented, including every Tauri command and `main`. Idiom departures worth naming:
process-global mutable statics; `String` as an error type across service boundaries; boolean
parameters (`acknowledged`, `consent`, `duplicate`) where an enum or request type would say what is
meant; and `Library` methods that take four positional strings.

---

## Implementation sequence

1. **Typed identifiers and structured errors** — the foundation everything else is expressed in.
2. **Installation service** — the first complete vertical, moving workflow out of Tauri.
3. **Application runtime and job lifecycle** — replaces the global statics with owned state.
4. **Account and signing split** — separates authentication from provisioning and signing.
5. **Storage boundaries** — metadata persistence apart from managed artifact operations.
6. **Testable external boundaries** — narrow interfaces plus deterministic fakes.
7. **Thin Tauri commands** — adapters only.
8. **Documentation and idiom audit** — to zero undocumented items.

Each step keeps the suite green.

---

## Dependency direction

```
        domain/            identifiers, structured errors — knows nothing else
          ▲
          │
     application/          workflows: installation service, runtime
          ▲                    (no tauri, no AppHandle, no State, no Channel)
          │
   ┌──────┴───────┬─────────────┬──────────────┐
library/      installation/   accounts/     signer/          infrastructure
   ▲              ▲              ▲             ▲
   └──────────────┴──────┬───────┴─────────────┘
                         │
                   src-tauri/               adapters: decode, call, map, subscribe
```

Nothing in `domain` or `application` names Tauri. `src-tauri` holds no workflow of its own: a
command decodes its inputs, validates identifiers, calls a service, and maps the result.

## State ownership

| State | Owner | Shared how | Lock |
|---|---|---|---|
| Manifest writer, lease table | `library::Library` | `Arc<Shared>`; cloning shares it | two blocking mutexes, `metadata` before `pins` |
| Installation gate, prepared review, live status | `application::installation::InstallationService` | `Arc`; cloning shares it | async gate, then a blocking mutex never held across `await` |
| Everything above | `application::runtime::Runtime` | constructed once at startup, cloned | none of its own |
| Account session, teams, certificate | `accounts::Accounts` | Tauri state | one blocking mutex plus an async operation gate |
| Device-log capture | `LogCapture` in `src-tauri` | Tauri state | its own async gate |

## Concurrency and cancellation rules

- **One installation or review at a time.** The gate refuses rather than queues: a person who
  clicks twice is told the first is still running.
- **Library removal and reclaim take the installation gate**, so a managed file cannot disappear
  while a review points at it or an installation is reading it.
- **A device-log capture excludes only another capture.** It is read-only and may run alongside
  anything else.
- **Lock ordering** is gate → service state → library metadata → library leases. No blocking guard
  is ever held across an `await`.
- **Cancellation stops an installation up to the moment iOS is asked to install, and not after.**
  Cancelling the Rust task is not the same as cancelling the installation; once the device has the
  command, Orbiter reports what it observes rather than what it intended.
- **A lease is released by `Drop`**, so a failed or panicking operation still gives its file back.

## Persistence and recovery rules

- Every stage change is journalled *before* it is announced, and recorded in the library's history.
- High-frequency byte progress is deliberately transient; only stage changes are durable.
- **A history-write failure is never reported as a failed installation.** Once iOS says it
  installed the app, that is the outcome; the note about bookkeeping is appended to the message.
- **Recovery never infers success.** An installation interrupted while iOS was installing becomes
  `Unknown`. Nothing is restarted, re-queued, or retried.
- A client that reconnects asks for a status snapshot, because the progress events it missed are
  gone. Nothing is replayed.
- A subscriber that disappears does not stop the work or lose its history.

See [the storage decision note](storage-decision.md) for why metadata is still JSON.

## Documentation audit

```
cargo run --offline -p doc-audit
```

Parses every Orbiter-owned Rust file with `syn` and reports each function, method, trait method,
inline module and file that carries no documentation. It exists because `missing_docs` only covers
the public API and a regular expression cannot tell a `fn` in a string from a real one.
