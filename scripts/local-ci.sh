#!/usr/bin/env bash
# local-ci.sh
# Run local CI checks before pushing
#
# Run: ./scripts/local-ci.sh
# Or via cargo alias: cargo lci

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}=== Local CI Checks ===${NC}"
echo ""

# Formatting
echo -e "${YELLOW}[1/5] Checking formatting...${NC}"
if cargo fmt --all -- --check; then
    echo -e "${GREEN}  Formatting OK${NC}"
else
    echo -e "${RED}  Formatting FAILED - run 'cargo fmt --all'${NC}"
    exit 1
fi

# TOML validation
echo -e "${YELLOW}[2/5] Validating TOML files...${NC}"
if command -v taplo &> /dev/null; then
    if taplo check; then
        echo -e "${GREEN}  TOML validation OK${NC}"
    else
        echo -e "${RED}  TOML validation FAILED${NC}"
        exit 1
    fi
else
    echo -e "${YELLOW}  Warning: taplo not installed, skipping TOML validation${NC}"
fi

# Clippy
echo -e "${YELLOW}[3/5] Running clippy...${NC}"
if cargo clippy --all-targets -- -D warnings; then
    echo -e "${GREEN}  Clippy OK${NC}"
else
    echo -e "${RED}  Clippy FAILED${NC}"
    exit 1
fi

# Tests
echo -e "${YELLOW}[4/5] Running tests...${NC}"
if cargo test --release; then
    echo -e "${GREEN}  Tests OK${NC}"
else
    echo -e "${RED}  Tests FAILED${NC}"
    exit 1
fi

# Doc tests (quick check that documentation compiles)
echo -e "${YELLOW}[5/5] Checking documentation...${NC}"
if cargo doc --no-deps --document-private-items 2>/dev/null; then
    echo -e "${GREEN}  Documentation OK${NC}"
else
    echo -e "${RED}  Documentation FAILED${NC}"
    exit 1
fi

echo ""
echo -e "${GREEN}=== All checks passed! ===${NC}"
