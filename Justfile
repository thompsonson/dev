# dev project task runner
# Install just: https://github.com/casey/just

# Trigger a stable release: opens the Release PR via release-please.
# Merging that PR creates the vX.Y.Z tag and builds the binaries.
# See docs/adr/001-release-automation.md
release-stable:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Triggering release-please workflow..."
    gh workflow run release-please.yml
    echo "Waiting for workflow to start..."
    sleep 5
    run_id=$(gh run list --workflow=release-please.yml --limit=1 --json databaseId --jq '.[0].databaseId')
    echo "Watching run $run_id..."
    gh run watch "$run_id" --exit-status
    echo ""
    echo "Release PR is open. Review and merge it to cut the stable release:"
    gh pr list --search "chore(main): release" --json url --jq '.[0].url' 2>/dev/null || true

# Tag and push a dev pre-release build
release-dev:
    #!/usr/bin/env bash
    set -euo pipefail
    tag="v$(grep '^version' dev-cli/Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')-dev.$(date +%Y%m%d).$(git rev-parse --short HEAD)"
    echo "Tagging $tag"
    git tag "$tag"
    git push origin "$tag"

# Install dev on this machine as a host (pop-mini)
install-host:
    curl -fsSL https://raw.githubusercontent.com/thompsonson/dev/main/scripts/bootstrap.sh | bash -s -- --host

# Install dev on this machine as a client of pop-mini (stable channel)
install-client:
    curl -fsSL https://raw.githubusercontent.com/thompsonson/dev/main/scripts/bootstrap.sh | DEV_HOST=pop-mini bash

# Install dev on this machine as a client of pop-mini (dev channel)
install-client-dev:
    curl -fsSL https://raw.githubusercontent.com/thompsonson/dev/main/scripts/bootstrap.sh | DEV_CHANNEL=dev DEV_HOST=pop-mini bash

# Run all tests
test:
    cargo test

# Run clippy
lint:
    cargo clippy

# Build release binary for the local target
build:
    cargo build --release
