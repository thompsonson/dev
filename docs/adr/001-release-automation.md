# ADR 001: Release automation with release-please

## Status

Accepted

## Context

The project produces a CLI binary (`dev`) distributed as prebuilt binaries via GitHub Releases. We need:

- Automated version bumping in `Cargo.toml`
- A `CHANGELOG.md` generated from commit history
- A stable release tag that triggers the binary build workflow
- A separate dev channel for pre-release builds

The project uses squash-merge only on `main`, so every merged PR produces exactly one commit. All PR titles must follow Conventional Commits — this is the input release-please uses to determine version bumps and changelog entries.

## Decision

Use **release-please** (`googleapis/release-please-action@v4`) for stable release automation.

### Configuration constraints

**release-please's Rust plugin requires explicit `version = "x.y.z"` in `[package]`.** It does not support Cargo workspace version inheritance (`version.workspace = true`). Sub-crates must carry explicit versions.

The package path in `release-please-config.json` and `.release-please-manifest.json` is `"dev-cli"` — the binary crate. `dev-lib/Cargo.toml` is updated on each release via `extra-files` jsonpath `$.package.version`.

### Release channels

| Channel | Tag pattern | GH Release type | Bootstrap default |
|---|---|---|---|
| stable | `vX.Y.Z` | Published (Latest) | yes |
| dev | `vX.Y.Z-dev.YYYYMMDD.HASH` | Pre-release | no (`--channel dev`) |

The release workflow detects `-dev.` in the tag name and passes `--prerelease` to `gh release create`.

### Stable release flow

1. Merge PRs to `main` using conventional commit titles (`feat:`, `fix:`, etc.)
2. release-please opens or updates a Release PR bumping both `Cargo.toml` files and `CHANGELOG.md`
3. Merge the Release PR — this creates the `vX.Y.Z` tag
4. The tag triggers `.github/workflows/release.yml`, which builds five targets and publishes the release

### Dev release flow

Manual tag:
```bash
git tag v0.0.5-dev.$(date +%Y%m%d).$(git rev-parse --short HEAD)
git push origin <tag>
```

## Alternatives considered

**Cargo-release** — automates version bumping and tagging but does not generate changelogs or open PRs. Requires manual invocation; less integrated with GitHub.

**Manual tagging with manual version bumps** — no tooling overhead but relies on discipline. Rejected because the release PR step enforces a review gate before a stable tag is created.

## Consequences

- All PR titles must be valid Conventional Commits — this is enforced by branch protection and the squash-merge policy.
- `dev-cli/Cargo.toml` and `dev-lib/Cargo.toml` versions are managed by release-please on stable cuts; do not edit them manually.
- `.release-please-manifest.json` records the last released version under the key `"dev-cli"`. If it drifts from `dev-cli/Cargo.toml`, reset it manually and commit to `main`.
- Workspace version inheritance (`version.workspace = true`) must not be used — it breaks the release-please Rust plugin.
