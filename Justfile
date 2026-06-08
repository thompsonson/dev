# dev project task runner
# Install just: https://github.com/casey/just

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
