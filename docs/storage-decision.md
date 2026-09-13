# Why the library still uses JSON, and when SQLite would be worth it

Orbiter's library keeps its metadata in one `manifest.json`, replaced atomically, beside
`artifacts/<sha256>.ipa` and `icons/<sha256>.png`. This records why that is still the right choice
and what would change the answer.

## The thing a database would not fix

**A database transaction cannot include the IPA files.** The manifest is small; the artifacts are
hundreds of megabytes each, and they live on the filesystem because that is where a 210 MB file
belongs. So the hard part of this design — publishing bytes and metadata such that a crash between
them leaves something recoverable — is exactly the same under SQLite as under JSON. It is solved
by ordering and by never deleting what a person did not ask to delete, not by transactions:

- **Import** stages a copy, hashes and inspects it outside the metadata lock, publishes the bytes,
  then writes the manifest. If the manifest write fails, the import removes the bytes it just
  published — an operation cleaning up after itself.
- **Removal** records the tombstone and the pending cleanup *first*, then deletes bytes. A failure
  after the record leaves files present and a pending removal to retry; a failure before it leaves
  everything as it was.
- **Unreferenced bytes** are counted and named, never swept. Startup finishes removals a person
  asked for and nothing else.

SQLite would move the small half of that problem and leave the large half untouched.

## What JSON costs today

One file, read and re-validated in full on every public library call, with an
`O(artifacts × attempts)` integrity pass. At the current bounds — 200 attempts per app, 2,000
overall, a manifest capped at 64 MiB — that is milliseconds, and the durable write is dominated by
one `fsync` rather than by parsing. The cost is real but it is not yet the thing to fix; the
measured 21-second integration test that writes 205 installations is ~410 `fsync` calls, not
parsing.

## What would make SQLite worthwhile

Any one of these:

1. **Concurrent writers.** Today exactly one Orbiter process may write one library, enforced by an
   in-process lock and documented as a constraint. A second window, a background agent, or a CLI
   run alongside the app would need real locking, and SQLite's is correct where a whole-file
   replace is not.
2. **Partial reads becoming necessary.** A history that no longer fits comfortably in memory, or a
   screen that needs to page or query rather than receive everything.
3. **Queries the current shape cannot answer cheaply** — "every install of this app across every
   device, newest first, limit 20" is a linear scan today.
4. **Growth past the current bounds.** If the per-app and overall caps have to rise by an order of
   magnitude, whole-file replacement stops being reasonable.

## If it happens

Not in the same change as a service extraction. A storage swap should be its own step, with the
existing schema migrated explicitly and versioned, IDs and relationships preserved, and artifact
files untouched — they are already content-addressed, which is the part that makes any migration
survivable.
