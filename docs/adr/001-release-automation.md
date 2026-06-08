# ADR 001: Release automation with release-please

## Status

Proposed

## Context

The project produces a CLI binary (`dev`) distributed as prebuilt binaries via GitHub Releases. We need:

- Automated version bumping in `Cargo.toml`
- A `CHANGELOG.md` generated from commit history
- A stable release tag that triggers the binary build workflow
- A separate dev channel for pre-release builds

The project uses squash-merge only on `main`, so every merged PR produces exactly one commit on `main`. All PR titles must follow Conventional Commits — this is the input release-please uses to determine version bumps and changelog entries.

Because the repo requires PRs for all commits to `main`, the Release PR is a structural necessity: release-please must bump `Cargo.toml` and `CHANGELOG.md`, and those file changes must land via a PR. This is not a process choice — it is a constraint of the branch protection model.

## Decision

Use **release-please** (`googleapis/release-please-action@v4`) for stable release automation, triggered **manually** via `just release-stable` rather than automatically on every push to `main`.

### Why manual trigger

release-please's default model triggers on every push to `main`, opening or updating a Release PR whenever a `feat:` or `fix:` PR lands. This produces constant Release PR noise and ties the stable release cadence to the development merge cadence.

The project goal is: merge to `main` freely, ship dev builds continuously, cut stable releases as a deliberate explicit act. Changing the workflow trigger to `workflow_dispatch` only — and exposing it as `just release-stable` — achieves this without changing any release-please internals.

### Release PR lifecycle

Since the repo uses squash-merge only, one merged PR = one commit on `main`. release-please maintains **one** Release PR at a time, accumulating all releasable PRs since the last stable release:

| Merged PR type | Effect |
|---|---|
| `feat:` or `fix:` | Included in the next Release PR when `just release-stable` is run |
| `chore:`, `refactor:`, `docs:`, `test:` | Never included — not releasable units |

A stable release may include many merged PRs. No Release PR is opened automatically — it only appears when `just release-stable` is explicitly run. Until then `main` keeps moving and the dev channel picks up changes via `just tag-dev`.

### Configuration constraints

**release-please's Rust plugin requires explicit `version = "x.y.z"` in `[package]`.** Cargo workspace version inheritance (`version.workspace = true`) is not supported. Sub-crates must carry explicit versions.

The package path in `release-please-config.json` and `.release-please-manifest.json` is `"dev-cli"`. `dev-lib/Cargo.toml` is updated on each release via `extra-files` jsonpath `$.package.version`.

### Release channels

| Channel | Tag pattern | GH Release type | Bootstrap default |
|---|---|---|---|
| stable | `vX.Y.Z` | Published (Latest) | yes |
| dev | `vX.Y.Z-dev.YYYYMMDD.HASH` | Pre-release | no (`--channel dev`) |

---

## Stable release flow

```mermaid
sequenceDiagram
    actor Dev
    participant J as just release-stable
    participant RP as release-please workflow<br/>(workflow_dispatch)
    participant RPR as Release PR
    participant main as main branch
    participant tag as vX.Y.Z tag
    participant RW as release workflow
    participant GHR as GitHub Release

    Note over Dev,GHR: Triggered explicitly — no automatic CI involvement

    Dev->>J: just release-stable
    J->>RP: gh workflow run release-please.yml
    RP->>RPR: open Release PR
    Note over RPR: bumps dev-cli/Cargo.toml<br/>bumps dev-lib/Cargo.toml<br/>prepends CHANGELOG.md<br/>updates .release-please-manifest.json<br/>accumulates all feat:/fix: PRs since last tag
    Dev->>RPR: review and merge
    RPR->>main: merge commit lands
    RPR->>tag: release-please creates vX.Y.Z tag
    tag->>RW: triggers release.yml
    RW->>GHR: create draft release
    RW->>GHR: build + upload 5 binaries (parallel)
    Note over RW,GHR: x86_64-linux-musl<br/>aarch64-linux-musl<br/>aarch64-linux-android<br/>x86_64-apple-darwin<br/>aarch64-apple-darwin
    RW->>GHR: publish release (Latest)
```

## Dev release flow

```mermaid
sequenceDiagram
    actor Dev
    participant J as just tag-dev
    participant tag as vX.Y.Z-dev.DATE.HASH tag
    participant RW as release workflow
    participant GHR as GitHub Release

    Note over Dev,GHR: Triggered explicitly at any time — no Release PR involved

    Dev->>J: just tag-dev
    J->>tag: create + push vX.Y.Z-dev.YYYYMMDD.HASH
    tag->>RW: triggers release.yml
    RW->>GHR: create draft pre-release
    Note over RW,GHR: detects -dev. in tag name<br/>passes --prerelease flag
    RW->>GHR: build + upload 5 binaries (parallel)
    RW->>GHR: publish pre-release (not Latest)
```

The dev channel is never picked up by `bootstrap.sh` default installs.

### Bootstrap commands

**Host (pop-mini) — stable, enable systemd daemon:**
```bash
curl -fsSL https://raw.githubusercontent.com/thompsonson/dev/main/scripts/bootstrap.sh | bash -s -- --host
```

**Client (Mac/Termux) — stable channel:**
```bash
curl -fsSL https://raw.githubusercontent.com/thompsonson/dev/main/scripts/bootstrap.sh | DEV_HOST=pop-mini bash
```

**Client (Mac/Termux) — dev channel:**
```bash
curl -fsSL https://raw.githubusercontent.com/thompsonson/dev/main/scripts/bootstrap.sh | DEV_CHANNEL=dev DEV_HOST=pop-mini bash
```

---

## Alternatives considered

**Automatic push-to-main trigger** — release-please's default. Opens a Release PR on every `feat:`/`fix:` merge. Rejected: ties stable release cadence to development cadence, produces constant Release PR noise, and gives no explicit stable release gate.

**`skip-github-pull-request: true`** — suppresses the Release PR and fires a tag/release automatically on every releasable commit. Rejected: fully automatic stable releases with no human gate.

**Knope** — supports `workflow_dispatch` releases with no PR at all. Not adopted: less mature, requires additional tooling.

**Cargo-release** — handles version bumping and tagging but does not generate changelogs. Rejected: no CHANGELOG automation.

**Manual tagging with manual version bumps** — no tooling, relies entirely on discipline. Rejected: no CHANGELOG and no version consistency enforcement.

## Consequences

- `just release-stable` is the only trigger for a stable release. No automation opens a Release PR without it.
- All PR titles must be valid Conventional Commits — enforced by squash-merge policy.
- `dev-cli/Cargo.toml` and `dev-lib/Cargo.toml` versions are managed by release-please on stable cuts; do not edit them manually.
- `.release-please-manifest.json` records the last released version under key `"dev-cli"`. If it drifts, reset it to match `dev-cli/Cargo.toml` and commit to `main`.
- Workspace version inheritance (`version.workspace = true`) must not be used — it breaks the release-please Rust plugin.
