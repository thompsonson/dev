# Release process

See [ADR 001](adr/001-release-automation.md) for the decision record and sequence diagrams.

## References

- [release-please documentation](https://github.com/googleapis/release-please)
- [release-please-action](https://github.com/googleapis/release-please-action)
- [Conventional Commits specification](https://www.conventionalcommits.org/)
- [Semantic Versioning](https://semver.org/)

## Channels

| Channel | Tag pattern | GitHub Release | Who gets it |
|---|---|---|---|
| **stable** | `vMAJOR.MINOR.PATCH` | Published, marked Latest | `bootstrap.sh` default |
| **dev** | `vX.Y.Z-dev.YYYYMMDD.COMMIT` | Pre-release, not Latest | `bootstrap.sh --channel dev` |

## Stable releases

Stable releases are triggered explicitly by the developer. The CI/CD pipeline does not decide when a stable release happens.

### How a stable release happens

1. Run `just release-stable` — triggers the release-please workflow, watches it complete, and prints the Release PR URL
2. Review the Release PR (version bump, CHANGELOG)
3. Merge the Release PR — release-please creates the `vX.Y.Z` tag
4. `release.yml` fires on the tag, builds five targets, publishes the GitHub Release

### Workflows

| Workflow | Trigger | What it does |
|---|---|---|
| `release-please.yml` | `just release-stable` (`workflow_dispatch`) | Opens the Release PR with version bump and CHANGELOG |
| `release.yml` | Tag push (`v*`) | Builds five binary targets, creates GitHub Release with assets |

### Commit message requirements

release-please derives the next version and CHANGELOG from [Conventional Commits](https://www.conventionalcommits.org/). The repo uses squash-merge only, so the PR title becomes the commit message on `main` — write PR titles in conventional format.

| Prefix | Semver bump | Example |
|---|---|---|
| `feat:` | minor | `feat(routing): add session URL scheme` |
| `fix:` | patch | `fix(bootstrap): retry on 504` |
| `feat!:` or `BREAKING CHANGE:` | major | `feat!: change config format` |
| `chore:`, `refactor:`, `docs:`, `test:` | none | `chore: update dependencies` |

### Cargo.toml versioning

release-please manages two files on each stable release:

- `dev-cli/Cargo.toml` — primary version file (`[package] version`)
- `dev-lib/Cargo.toml` — updated via `extra-files` jsonpath `$.package.version`

Do not edit them manually between releases — a CI check is planned to enforce this (see [#37](https://github.com/thompsonson/dev/issues/37)).

### Configuration files

| File | Purpose |
|---|---|
| `release-please-config.json` | Package to track, changelog sections, extra-files |
| `.release-please-manifest.json` | release-please internal state — current stable version under key `"dev-cli"` |

If `.release-please-manifest.json` drifts from `dev-cli/Cargo.toml`, set `"dev-cli"` to the current version and commit directly to `main`.

## Dev releases

Dev builds are created as and when the developer needs, with no changelog. Use `just release-dev`:

```bash
just release-dev
```

Or manually:

```bash
git tag v0.0.5-dev.$(date +%Y%m%d).$(git rev-parse --short HEAD)
git push origin <tag>
```

The release workflow detects `-dev.` in the tag name and creates a **pre-release** GitHub Release (not marked Latest). The bootstrap installer's stable channel ignores pre-releases.

## Troubleshooting

**release-please fails with `value at path package.version is not tagged`**
The Rust plugin requires `version = "x.y.z"` in `[package]`. Workspace inheritance (`version.workspace = true`) is not supported. Sub-crates must carry explicit versions.

**release-please opens no PR**
All commits since the last tag use non-releasing prefixes (`chore:`, `docs:`, `refactor:`). release-please only opens a PR for `feat:` or `fix:` commits.

**`.release-please-manifest.json` is out of sync**
Set `"dev-cli"` to match `dev-cli/Cargo.toml` version and commit to `main`.
