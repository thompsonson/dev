# CI Required Gate Flow

Branch protection should require one always-emitted check named `ci`, not individual conditional jobs.

This keeps branch protection reliable while allowing the workflow to skip irrelevant work for docs-only or config-only PRs.

## Flow

```mermaid
sequenceDiagram
    actor Dev
    participant PR as Pull request
    participant Changes as changes job
    participant Rust as rust jobs
    participant Docs as docs jobs
    participant Gate as ci gate job
    participant BP as Branch protection

    Dev->>PR: Open or update PR
    PR->>Changes: Detect changed paths

    alt Rust-relevant files changed
        Changes->>Rust: Run version guard, fmt, clippy, tests
        Rust-->>Gate: Report pass/fail
    else No Rust-relevant files changed
        Changes-->>Gate: Rust jobs not required
    end

    alt Markdown/docs files changed
        Changes->>Docs: Run docs checks
        Docs-->>Gate: Report pass/fail
    else No docs files changed
        Changes-->>Gate: Docs jobs not required
    end

    Gate->>Gate: Evaluate relevant job results

    alt Any relevant job failed or was cancelled
        Gate-->>BP: ci = failure
        BP-->>PR: Block merge
    else All relevant jobs passed or were skipped intentionally
        Gate-->>BP: ci = success
        BP-->>PR: Merge allowed
    end
```

## Decision Points

- Required checks must always be emitted for every protected PR.
- Conditional jobs may be skipped, but skipped jobs must not be directly required by branch protection.
- Branch protection should require only the final `ci` gate check.
- The `ci` gate must fail if any relevant job fails or is cancelled.
- Path detection decides which job groups are relevant for a PR.
- Docs-only PRs should not run Rust tests unless Rust/Cargo/release files changed.

## Relevant File Groups

Rust CI is relevant when these files change:

- `**/*.rs`
- `**/Cargo.toml`
- `**/Cargo.lock`
- `.release-please-manifest.json`

Docs checks are relevant when these files change:

- `**/*.md`
- `docs/**`

## Branch Protection

Require:

- `ci`

Do not require directly:

- `version consistency`
- `test (ubuntu-latest)`
- `test (macos-latest)`

Those checks may remain visible on PRs, but they should be implementation details behind the required gate.
