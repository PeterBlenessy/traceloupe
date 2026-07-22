# Code signing (so Keychain-stored backup passwords survive rebuilds)

## Why this matters

An encrypted backup's password is stored in the macOS **Keychain** (`secret.rs`)
so the app can reopen the backup and decrypt full-resolution photos / run native
re-imports without re-prompting.

A Keychain item's access is gated by the **code signature** of the app that
created it. A dev build that is **unsigned or ad-hoc-signed** gets a *different*
identity every time it's rebuilt, so after a rebuild the new binary is no longer
trusted by the item's ACL. The symptoms:

- Re-import fails with `file is not a database` (the encrypted `Manifest.db` was
  opened as plaintext because no keys loaded).
- Full-resolution photos don't open (thumbnails still work — they're decrypted to
  disk at import time).
- On opening a backup you'll see a warning in the console:
  *"Backup is encrypted but its keys couldn't be loaded from the Keychain…"*

The fix is to sign every build with **one stable identity** so the item's ACL
keeps trusting the app across rebuilds. A free **self-signed** certificate is
enough for local development (no Apple Developer account needed).

## One-time setup

### 1. Create a self-signed code-signing certificate

Keychain Access → **Certificate Assistant ▸ Create a Certificate…**

- **Name:** `TraceLoupe Dev`
- **Identity Type:** Self Signed Root
- **Certificate Type:** **Code Signing**
- Create, and leave it in your **login** keychain.

(You can script this, but the Certificate Assistant is the reliable path.)

### 2. Point the build at it

Tauri signs the app bundle with the identity named in `APPLE_SIGNING_IDENTITY`:

```sh
export APPLE_SIGNING_IDENTITY="TraceLoupe Dev"
```

Put that in your shell profile (or prepend it to the build command) so **every**
build uses the same identity — that's the whole point.

### 3. Build a signed app bundle and run it

```sh
pnpm app:build:dev      # debug .app, signed with the identity above
# → open src-tauri/target/debug/bundle/macos/TraceLoupe*.app
```

Verify the signature is your stable cert:

```sh
codesign -dv --verbose=4 "src-tauri/target/debug/bundle/macos/TraceLoupe Dev.app"
# Authority=TraceLoupe Dev   (not "adhoc")
```

### 4. Approve the Keychain item once

The first time the signed app reads the stored password, macOS asks to allow it.
Click **Always Allow**. Because the signing identity is now stable, that approval
**persists across rebuilds** — no more re-typing the password.

## Notes

- **`pnpm app:dev` (`tauri dev`) is ad-hoc-signed** and will *not* persist keys
  across rebuilds. Use a signed **`app:build:dev`** bundle when testing encrypted
  backups; `tauri dev` is fine for everything else.
- Re-importing a backup with its password re-stores the key under the current
  identity, so it's always a valid recovery path.
- This stable-signing setup is also the **prerequisite for Touch ID** (Phase 1):
  biometric-gated Keychain items and `LAContext` require a properly signed app.
  See the Touch ID plan in the project notes.
- For distribution (not local dev) you'd use an Apple **Developer ID** identity
  instead of a self-signed one; the mechanism is identical.
