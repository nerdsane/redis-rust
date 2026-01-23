#!/bin/bash
# Setup git hooks for redis-rust development
#
# Run this once after cloning:
#   ./scripts/setup-hooks.sh

set -e

REPO_ROOT="$(git rev-parse --show-toplevel)"

echo "Setting up git hooks..."

# Configure git to use our hooks directory
git config core.hooksPath "$REPO_ROOT/.githooks"

echo "Git hooks installed!"
echo ""
echo "Pre-commit hook will now check:"
echo "  - cargo fmt --all --check"
echo ""
echo "To skip hooks temporarily: git commit --no-verify"
