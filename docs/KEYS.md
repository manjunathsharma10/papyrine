# Signing keys: ceremony, custody, rotation, compromise

Implements ARCHITECTURE section 9.3 and ADR-023. Code: `crates/papyrine-update`
(verification, in the app) and `tools/release-sign` (signing, never shipped).

**Status (v0.1 development): the production root keys do NOT exist yet.**
Generating them needs the owner's hardware security keys, so the ceremony below
is **PENDING OWNER ACTION**. Until it is done:

- only the INSECURE dev root exists (deterministic, public; seed phrase in
  `crates/papyrine-update/src/roots.rs`), usable only with the `dev-root` cargo
  feature, which `compile_error!`s in release builds (`debug_assertions` off);
- `PRODUCTION_ROOTS` in `roots.rs` is empty, so a release build verifies
  nothing and the update check can never show a banner (fail closed);
- nothing signed is published to GitHub Releases.

## Trust chain

```
2 root public keys (embedded in the app)
  -> keyring.json            signed by ONE root (either suffices)
       lists online keys: id, purpose (release | component), not_before, not_after
       lists revocations; carries a strictly increasing serial
    -> updates.json          signed by a keyring "release" key valid now
         per-installer SHA-256 + size; serial; expires
      -> installer           SHA-256 and size checked while streaming
```

Envelope: `{"signed": "<exact JSON text>", "signatures": [{"key_id", "sig"}]}`.
The signature covers `"<domain>\n" + signed` with domain `papyrine-keyring-v1`
or `papyrine-updates-v1`, so one file type's signature cannot be replayed as
another, and there is no canonicalisation step.

Client rules: a revoked, not-yet-valid, expired or wrong-purpose key is
rejected; the verified keyring with the highest serial among fetched, cached and
app-bundled wins (a replayed old keyring cannot un-revoke a key); an
`updates.json` or keyring older than the accepted serial is a rollback and is
ignored; an expired `updates.json` is ignored.

## Key inventory

| Key | Signs | Where it lives | Rotation |
|---|---|---|---|
| Root primary + root backup (Ed25519) | `keyring.json` only | Offline; one per hardware security key, stored in different places; encrypted paper seed backup of each in a safe | Replaced by an app release embedding the new root, signed by the other root |
| Release key | `updates.json` | `PAPYRINE_SIGN_KEY` secret in the protected GitHub `release` environment | Yearly or on suspicion |
| Component key | `components.json`, component manifests | Same environment, separate secret | Same |
| Tauri updater key (v1.0) | updater artifacts (minisign) | Same environment | Ship the new key in a release signed with the old |

## Root key ceremony (PENDING OWNER ACTION)

Needs: an air-gapped machine (fresh boot of a live OS, no network), two hardware
security keys that support Ed25519 (YubiKey 5.2.3+ via PIV or OpenPGP), two
people if available (a witness), a safe, acid-free paper and a printer that is
not network-connected.

1. Boot the air-gapped machine; verify no network interface is up.
2. Build `release-sign` from the tagged commit on a networked machine, copy the
   binary and `sha256sum` over on a USB stick, verify the hash offline.
3. For each root (`root-primary`, `root-backup`):
   `release-sign gen-key --out root-primary.seed` (file is created 0600 and
   refuses to overwrite). Note the printed base64 public key.
4. Import each seed into its own hardware key (or, for software signing during
   the ceremony, keep the seed only on the air-gapped machine's RAM disk).
   Print each seed as hex on paper, seal it, and store it in the safe. Do not
   photograph or type it anywhere else.
5. Wipe the seed files (`shred`/RAM disk reboot).
6. Write the two public keys into `PRODUCTION_ROOTS` in
   `crates/papyrine-update/src/roots.rs` (ids `root-primary`, `root-backup`) and
   open a PR; record both public keys and the ceremony date, witness and
   serials of the hardware keys at the bottom of this file.
7. Create the first online release key (`gen-key` in the protected release
   environment or locally, then store the seed as the `PAPYRINE_SIGN_KEY`
   environment secret and delete the local copy).
8. On the air-gapped machine, sign the first keyring:
   `release-sign sign-keyring --in keyring.payload.json --id root-primary --key-file root-primary.seed --out keyring.json`
   (payload: `schema` 1, `serial` 1, the release key with `not_before`/`not_after`
   at most 12 months apart, `revoked` empty). Verify with
   `release-sign verify-keyring keyring.json --root-id root-primary --root-pub <b64>`.
9. Publish `keyring.json` as a release asset and bundle the same file in the app.

Only after step 9 may anything signed be published.

## Routine release signing

In the protected `release` environment (required reviewer = owner, main branch
only, no fork PRs): the job builds installers, computes SHA-256 and size, fills
`updates.json` payload (`serial` +1, `expires` about 60 days out, `severity`,
`affected` semver ranges), and runs
`release-sign sign-updates --in updates.payload.json --id <release key id> --out updates.json`
with the key from `PAPYRINE_SIGN_KEY`. The job then runs `verify-updates`
against the published keyring before uploading `updates.json` and
`keyring.json` as release assets. Keys are never passed on a command line.

## Rotation

- **Release/component key (yearly):** generate the new key, add it to the
  keyring with a validity window that starts before the old one ends, raise
  `serial`, sign with a root (offline), publish and re-bundle. The old key stays
  valid until its `not_after`; remove it from the next keyring after that.
- **Root key:** generate the replacement on the air-gapped machine, embed the new
  public key in an app release, and sign that release's keyring with the
  remaining root. Retire the old root from `PRODUCTION_ROOTS` in the same
  release. Either root alone can sign a keyring, so losing one is survivable
  but it must be replaced promptly.

## Compromise response

1. **Online key suspected:** sign a keyring (offline root) that lists the key in
   `revoked`, with a higher `serial`; publish it immediately and bundle it in the
   next release. Clients that fetch it stop trusting the key at once; offline
   clients learn on their next check. Rotate in a replacement key in the same
   keyring.
2. Pull any `updates.json` signed by that key and publish a fresh one signed by
   the new key with a higher `serial`.
3. Publish an advisory (SECURITY.md process) listing the window of exposure.
4. **Root key lost:** use the backup root to ship a release with a new root.
   **Root key stolen:** treat as full compromise: ship a release embedding two
   new roots signed by the surviving root, tell users to install it manually
   (clients with only the stolen root cannot be recalled), and revoke trust
   announcements through all channels.
5. Post-incident: update this file and add a dated entry below.

## Records

| Date | Event | Notes |
|---|---|---|
| (pending) | Root ceremony | Owner action; public keys and witness recorded here |
