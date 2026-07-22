# Releasing TraceLoupe

Pre-1.0, the **minor** version tracks a milestone (see the table in
[`CHANGELOG.md`](CHANGELOG.md)); patches are fixes/small additions on top. A
release is a normal commit + PR — there is no separate publish step.

## Cut a release

1. **Bump the version.** One command keeps all the manifests in step:

   ```bash
   scripts/release.sh 0.30.0
   ```

   It edits `package.json` (the source of truth), the workspace `Cargo.toml`,
   `src-tauri/tauri.conf.json`, and refreshes `Cargo.lock`.

2. **Write the CHANGELOG.** Add a `## [0.30.0] — YYYY-MM-DD` section describing
   what shipped — for a **new minor**, open it with a one-line **bold milestone
   summary** so the section headers read as a milestone index. Reset
   `## [Unreleased]` to `_Nothing yet._`.

   > Tip: `git log <last-release-commit>..origin/main` shows everything that has
   > accumulated since the previous version bump — the raw material for the entry.

3. **Verify**, commit, and open a PR:

   ```bash
   scripts/check-releases.sh          # all invariants must pass
   git commit -am "Release v0.30.0 — <theme>"
   ```

4. **Merge to main.** That's it — **the tag is created automatically** by
   [`.github/workflows/release-tag.yml`](.github/workflows/release-tag.yml) as
   soon as the bump lands on main. No manual `git tag` step.

## What can't slip through

Two guards enforce the invariants that were missed repeatedly before:

- **No bump without a CHANGELOG entry.** The `Release invariants` job in
  [`ci.yml`](.github/workflows/ci.yml) runs `scripts/check-releases.sh` on every
  PR and fails if `package.json`'s version has no `## [version]` CHANGELOG
  section (or if the manifests disagree, or a past documented version lost its
  tag).
- **No release without a tag.** When a version bump reaches main, the
  `Tag release` workflow creates and pushes `v<version>` automatically (and
  refuses to tag if the CHANGELOG entry is somehow absent).

Run `scripts/check-releases.sh` anytime to audit version ↔ CHANGELOG ↔ tag
consistency locally.
