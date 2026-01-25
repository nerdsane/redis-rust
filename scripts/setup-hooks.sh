#!/usr/bin/env bash
# setup-hooks.sh
# Setup development environment for redis-rust
#
# Run once after cloning:
#   ./scripts/setup-hooks.sh

set -euo pipefail

echo "=== redis-rust Development Setup ==="
echo ""

# Check for Python/pip
echo "[1/4] Setting up pre-commit..."
if command -v pip3 &> /dev/null; then
    pip3 install --quiet pre-commit
elif command -v pip &> /dev/null; then
    pip install --quiet pre-commit
else
    echo "  Warning: pip not found, skipping pre-commit installation"
    echo "  Install manually: pip install pre-commit"
fi

# Install pre-commit hooks if pre-commit is available
if command -v pre-commit &> /dev/null; then
    pre-commit install
    echo "  Pre-commit hooks installed"
else
    echo "  Warning: pre-commit not in PATH"
fi

# Install Rust tools
echo "[2/4] Installing Rust tools..."
cargo install --quiet taplo-cli 2>/dev/null || echo "  taplo-cli already installed or failed"
cargo install --quiet cargo-machete 2>/dev/null || echo "  cargo-machete already installed or failed"
cargo install --quiet bacon 2>/dev/null || echo "  bacon already installed or failed"

# Check for optional tools
echo "[3/4] Checking optional tools..."
if command -v sccache &> /dev/null; then
    echo "  sccache: installed"
else
    echo "  sccache: not installed (optional, speeds up compilation)"
    echo "    Install: cargo install sccache (or brew install sccache)"
fi

if command -v mold &> /dev/null; then
    echo "  mold: installed"
else
    echo "  mold: not installed (optional, faster linking on Linux)"
    echo "    Install: apt install mold (Debian/Ubuntu)"
fi

# Legacy git hooks support (fallback)
echo "[4/4] Configuring git..."
REPO_ROOT="$(git rev-parse --show-toplevel)"
if [ -d "$REPO_ROOT/.githooks" ]; then
    git config core.hooksPath "$REPO_ROOT/.githooks"
    echo "  Legacy .githooks configured as fallback"
fi

echo ""
echo "=== Setup complete! ==="
echo ""
echo "Available commands:"
echo "  cargo lci           Run local CI checks"
echo "  cargo cl            Run clippy with strict warnings"
echo "  cargo ca            Check all targets"
echo "  bacon               Background checker (press 'l' for clippy, 't' for tests)"
echo ""
echo "Pre-commit hooks will automatically run:"
echo "  - cargo fmt"
echo "  - cargo clippy"
echo "  - taplo check"
