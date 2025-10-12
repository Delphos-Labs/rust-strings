#!/bin/bash
set -e

# Build script for Python wheels on Linux (amd64 and aarch64)
# Requires Docker to be installed and running
# Uses the official PyO3 maturin Docker images with multi-platform support

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[0;33m'
NC='\033[0m' # No Color

echo -e "${BLUE}Building rust-strings Python wheels${NC}"
echo "======================================"

# Create dist directory if it doesn't exist
mkdir -p dist

# Build for x86_64 (amd64) using native platform image
if ls dist/*manylinux*x86_64.whl 1> /dev/null 2>&1; then
    echo -e "\n${YELLOW}Skipping x86_64 (amd64) build - wheels already exist${NC}"
else
    echo -e "\n${GREEN}Building for x86_64 (amd64)...${NC}"
    docker run --rm --platform linux/amd64 \
        -v "$(pwd)":/io \
        -w /io \
        ghcr.io/pyo3/maturin \
        build --release \
        --features python_bindings \
        --manylinux 2014 \
        --strip \
        -i python3.8 python3.9 python3.10 python3.11 python3.12 \
        -o dist
fi

# Build for aarch64 (ARM64) using native platform image
if ls dist/*manylinux*aarch64.whl 1> /dev/null 2>&1; then
    echo -e "\n${YELLOW}Skipping aarch64 (ARM64) build - wheels already exist${NC}"
else
    echo -e "\n${GREEN}Building for aarch64 (ARM64)...${NC}"
    echo -e "${YELLOW}Note: Emulation may take several minutes per wheel${NC}"
    docker run --rm --platform linux/arm64 \
        -v "$(pwd)":/io \
        -w /io \
        ghcr.io/pyo3/maturin \
        build --release \
        --features python_bindings \
        --manylinux 2014 \
        --strip \
        -i python3.8 python3.9 python3.10 python3.11 python3.12 \
        -o dist
fi

# Build for macOS ARM64 (native build if on macOS)
if [[ "$OSTYPE" == "darwin"* ]]; then
    if ls dist/*macosx*arm64.whl 1> /dev/null 2>&1; then
        echo -e "\n${YELLOW}Skipping macOS ARM64 Python build - wheels already exist${NC}"
    else
        echo -e "\n${GREEN}Building for macOS ARM64 (Apple Silicon)...${NC}"

        # Check if maturin is installed
        if ! command -v maturin &> /dev/null; then
            echo -e "${YELLOW}maturin not found, installing...${NC}"
            pip install maturin
        fi

        # Build for macOS ARM64
        maturin build --release \
            --features python_bindings \
            --strip \
            --target aarch64-apple-darwin \
            -i python3.8 python3.9 python3.10 python3.11 python3.12 \
            -o dist
    fi
else
    echo -e "\n${YELLOW}Skipping macOS ARM64 build (not running on macOS)${NC}"
fi

# Build CLI binaries
echo -e "\n${BLUE}Building CLI binaries${NC}"
echo "======================================"

# Create dist directory for CLI binaries
mkdir -p dist/cli

# Build for macOS ARM64 (native build if on macOS)
if [[ "$OSTYPE" == "darwin"* ]]; then
    if [ -f "dist/cli/rust-strings-aarch64-apple-darwin" ]; then
        echo -e "\n${YELLOW}Skipping macOS ARM64 CLI build - binary already exists${NC}"
    else
        echo -e "\n${GREEN}Building CLI for macOS ARM64 (native build)...${NC}"
        cargo build --release --features cli --target aarch64-apple-darwin
        cp target/aarch64-apple-darwin/release/rust-strings dist/cli/rust-strings-aarch64-apple-darwin
    fi
else
    echo -e "\n${YELLOW}Skipping macOS CLI builds (requires macOS or osxcross setup)${NC}"
    echo -e "${YELLOW}Note: Cross-compiling for macOS from Linux requires additional setup with osxcross${NC}"
fi

# Build for Linux
echo -e "\n${GREEN}Building CLI for Linux platforms...${NC}"

# Skip Linux builds if SKIP_LINUX_CLI is set
if [ "${SKIP_LINUX_CLI}" = "1" ]; then
    echo -e "\n${YELLOW}Skipping Linux CLI builds (SKIP_LINUX_CLI=1)${NC}"
elif [[ "$OSTYPE" != "darwin"* ]] && [[ "$OSTYPE" == "linux-gnu"* ]]; then
    # Native Linux builds when on Linux
    echo -e "${BLUE}Building Linux CLI natively${NC}"

    # Build for Linux x86_64
    if [ -f "dist/cli/rust-strings-x86_64-linux" ]; then
        echo -e "\n${YELLOW}Skipping Linux x86_64 CLI build - binary already exists${NC}"
    else
        echo -e "\n${GREEN}Building CLI for Linux x86_64 (native build)...${NC}"
        cargo build --release --features cli --target x86_64-unknown-linux-gnu && \
        cp target/x86_64-unknown-linux-gnu/release/rust-strings dist/cli/rust-strings-x86_64-linux || \
        echo -e "${YELLOW}x86_64-linux build failed${NC}"
    fi

    # Build for Linux aarch64 (using Docker for cross-arch)
    if [ -f "dist/cli/rust-strings-aarch64-linux" ]; then
        echo -e "\n${YELLOW}Skipping Linux aarch64 CLI build - binary already exists${NC}"
    else
        if docker info > /dev/null 2>&1; then
            echo -e "\n${GREEN}Building CLI for Linux aarch64 (using Docker)...${NC}"
            docker run --rm \
                -v "$(pwd)":/project \
                -w /project \
                --platform linux/arm64 \
                rust:latest \
                sh -c "rustup target add aarch64-unknown-linux-gnu && cargo build --release --features cli --target aarch64-unknown-linux-gnu" && \
            cp target/aarch64-unknown-linux-gnu/release/rust-strings dist/cli/rust-strings-aarch64-linux || \
            echo -e "${YELLOW}aarch64-linux build failed${NC}"
        else
            echo -e "\n${YELLOW}Docker not running, trying native aarch64 build...${NC}"
            cargo build --release --features cli --target aarch64-unknown-linux-gnu && \
            cp target/aarch64-unknown-linux-gnu/release/rust-strings dist/cli/rust-strings-aarch64-linux || \
            echo -e "${YELLOW}aarch64-linux build failed (may need: rustup target add aarch64-unknown-linux-gnu)${NC}"
        fi
    fi
elif ! docker info > /dev/null 2>&1; then
    echo -e "\n${YELLOW}Docker is not running. Skipping Linux CLI builds.${NC}"
    echo -e "${YELLOW}To build Linux binaries:${NC}"
    echo -e "${YELLOW}  1. Start Docker${NC}"
    echo -e "${YELLOW}  2. Run this script again${NC}"
    echo -e "${YELLOW}Or set SKIP_LINUX_CLI=1 to skip Linux builds${NC}"
else
    # Use Docker for Linux builds when on macOS
    echo -e "${BLUE}Using Docker for Linux builds${NC}"

    # Build for Linux x86_64
    if [ -f "dist/cli/rust-strings-x86_64-linux" ]; then
        echo -e "\n${YELLOW}Skipping Linux x86_64 CLI build - binary already exists${NC}"
    else
        echo -e "\n${GREEN}Building CLI for Linux x86_64 (using Docker)...${NC}"
        docker run --rm \
            -v "$(pwd)":/project \
            -w /project \
            --platform linux/amd64 \
            rust:latest \
            cargo build --release --features cli --target x86_64-unknown-linux-gnu && \
        cp target/x86_64-unknown-linux-gnu/release/rust-strings dist/cli/rust-strings-x86_64-linux || \
        echo -e "${YELLOW}x86_64-linux build failed${NC}"
    fi

    # Build for Linux aarch64
    if [ -f "dist/cli/rust-strings-aarch64-linux" ]; then
        echo -e "\n${YELLOW}Skipping Linux aarch64 CLI build - binary already exists${NC}"
    else
        echo -e "\n${GREEN}Building CLI for Linux aarch64 (using Docker)...${NC}"
        docker run --rm \
            -v "$(pwd)":/project \
            -w /project \
            --platform linux/arm64 \
            rust:latest \
            sh -c "rustup target add aarch64-unknown-linux-gnu && cargo build --release --features cli --target aarch64-unknown-linux-gnu" && \
        cp target/aarch64-unknown-linux-gnu/release/rust-strings dist/cli/rust-strings-aarch64-linux || \
        echo -e "${YELLOW}aarch64-linux build failed${NC}"
    fi
fi

echo -e "\n${GREEN}Build complete!${NC}"
echo "Wheels built:"
ls -lh dist/*.whl 2>/dev/null || echo "No wheels found in dist/"
echo ""
echo "Total wheels: $(ls dist/*.whl 2>/dev/null | wc -l)"
echo ""
echo "CLI binaries built:"
ls -lh dist/cli/* 2>/dev/null || echo "No CLI binaries found"
