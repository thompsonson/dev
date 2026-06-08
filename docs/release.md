# Release process

## Channels

| Channel | Tag pattern | GitHub Release | Who gets it |
|---|---|---|---|
| **stable** | `vMAJOR.MINOR.PATCH` | Published, marked Latest | `bootstrap.sh` default |
| **dev** | `vX.Y.Z-dev.YYYYMMDD.COMMIT` | Pre-release, not Latest | `bootstrap.sh --channel dev` |

## Stable releases — automated via release-please

Stable releases are fully automated. The only manual step is merging the Release PR.

### How it works

1. Every merge to `main` triggers `.github/workflows/release-please.yml`.
2. release-please scans commits since the last release tag and builds a Release PR if there is anything to ship.
3. The Release PR bumps `dev-cli/Cargo.toml` and `dev-lib/Cargo.toml` versions, and prepends a section to `CHANGELOG.md`.
4. Merging the Release PR creates a `vX.Y.Z` tag on that commit.
5. The tag triggers `.github/workflows/release.yml`, which builds five targets and publishes a GitHub Release with binaries.

### Commit message requirements

release-please derives the next version and CHANGELOG from [Conventional Commits](https://www.conventionalcommits.org/). All commits reaching `main` **must** use a conventional prefix. Because the repo is configured for squash-merge only, the PR title becomes the commit message — write the PR title in conventional format.

| Prefix | Semver bump | Example |
|---|---|---|
| `feat:` | minor | `feat(routing): add session URL scheme` |
| `fix:` | patch | `fix(bootstrap): retry on 504` |
| `feat!:` or `BREAKING CHANGE:` | major | `feat!: change config format` |
| `chore:`, `refactor:`, `docs:`, `test:` | none (no release) | `chore: update dependencies` |

### Cargo.toml versioning

release-please manages two files on each stable release:

- `dev-cli/Cargo.toml` — primary version file (`[package] version`)
- `dev-lib/Cargo.toml` — updated via `extra-files` jsonpath `$.package.version`

Both must always be in sync. Do not edit them manually between releases.

### Configuration files

| File | Purpose |
|---|---|
| `release-please-config.json` | Tells release-please which package to track, changelog sections, and extra-files to update |
| `.release-please-manifest.json` | release-please's internal state — records the current stable version. Must equal the version in `dev-cli/Cargo.toml`. |

**Important:** `.release-please-manifest.json` uses the package path key `"dev-cli"` (not `"."`). If you ever recreate or reset the manifest, set it to match the current `dev-cli/Cargo.toml` version.

## Dev releases — manual

Dev releases are tagged manually to test in-progress work without cutting a stable release.

```bash
# tag format: vX.Y.Z-dev.YYYYMMDD.COMMITHASH
git tag v0.0.5-dev.$(date +%Y%m%d).$(git rev-parse --short HEAD)
git push origin <tag>
```

The release workflow detects `-dev.` in the tag name and creates a **pre-release** GitHub Release (not marked Latest). The bootstrap installer's stable channel ignores pre-releases.

Install a dev build:

```bash
curl -fsSL https://raw.githubusercontent.com/thompsonson/dev/main/scripts/bootstrap.sh | DEV_CHANNEL=dev bash
```

## Troubleshooting

**release-please fails with `value at path package.version is not tagged`**
The Rust plugin expects `version = "x.y.z"` inside a `[package]` block. Workspace inheritance (`version.workspace = true`) is not supported. Sub-crates must have explicit versions.

**release-please opens no PR after a merge**
All commits since the last tag use non-releasing prefixes (`chore:`, `docs:`, `refactor:` etc.). This is correct — release-please only opens a PR when there is a `feat:` or `fix:` commit to ship.

**`.release-please-manifest.json` is out of sync**
Set `"dev-cli"` to the current version in `dev-cli/Cargo.toml` and commit directly to `main`.
