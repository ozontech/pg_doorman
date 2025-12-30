#!/usr/bin/env bash
set -e

# Get project root (two levels up from tests/nix)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# Function to compute image tag from flake files (same logic as in GitHub workflow)
compute_flake_tag() {
    local flake_hash
    flake_hash=$(cat "${SCRIPT_DIR}/flake.nix" "${SCRIPT_DIR}/flake.lock" | shasum -a 256 | cut -c1-16)
    echo "flake-${flake_hash}"
}

# Configuration
REGISTRY="${REGISTRY:-ghcr.io}"
REPO="${REPO:-$(git config --get remote.origin.url | sed 's/.*://;s/.git$//')}"
IMAGE_NAME="${REGISTRY}/${REPO}/test-runner"
# Use flake-based tag by default (matches GitHub workflow), can be overridden with IMAGE_TAG env var
IMAGE_TAG="${IMAGE_TAG:-$(compute_flake_tag)}"
FULL_IMAGE="${IMAGE_NAME}:${IMAGE_TAG}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if Docker is available
if ! command -v docker &> /dev/null; then
    log_error "Docker is not installed or not in PATH"
    exit 1
fi

# Function to pull the latest image
pull_image() {
    log_info "Pulling test image: ${FULL_IMAGE}"
    if docker pull "${FULL_IMAGE}"; then
        log_info "Image pulled successfully"
    else
        log_warn "Failed to pull image from registry"
        log_error "  1. You're authenticated: docker login ${REGISTRY}"
        log_error "  2. The image exists: ${FULL_IMAGE}"
        log_error "  3. You have access to the repository"
        exit 1
    fi
}

# Function to try pulling image, but continue if local image exists
try_pull_image() {
    # Check if image exists locally
    if docker image inspect "${FULL_IMAGE}" &> /dev/null; then
        log_info "Local image found: ${FULL_IMAGE}"
        log_info "Attempting to update from registry..."
        if docker pull "${FULL_IMAGE}" 2>&1 | grep -q "denied\|unauthorized\|not found"; then
            log_warn "Cannot access registry, using local image"
        elif docker pull "${FULL_IMAGE}"; then
            log_info "Image updated successfully"
        else
            log_warn "Pull failed, using local image"
        fi
    else
        log_info "No local image found, pulling from registry..."
        if docker pull "${FULL_IMAGE}"; then
            log_info "Image pulled successfully"
        else
            log_error "Failed to pull image and no local image available"
            log_error "Please run 'make local-build' to build the image locally"
            log_error "Or authenticate: docker login ${REGISTRY}"
            exit 1
        fi
    fi
}

# Function to run a command in the container
run_in_container() {
    local cmd="$1"
    local interactive="${2:-false}"

    docker_args=(
        --rm
        -it
        --init
        -v "${PROJECT_ROOT}:/workspace"
        -w /workspace
        --network host
        --tmpfs /tmp:exec,mode=1777
        -e "POSTGRES_HOST=127.0.0.1"
        -e "POSTGRES_PORT=5432"
    )

    if [ "$interactive" = "true" ]; then
        docker_args+=(-i)
    fi

    # Add persistent volumes for caching
    docker_args+=(
        -v pg_doorman_cargo_cache:/root/.cargo/registry
        -v pg_doorman_cargo_git:/root/.cargo/git
        -v pg_doorman_go_cache:/root/go/pkg/mod
        -v pg_doorman_go_build:/root/.cache/go-build
        -v pg_doorman_npm_cache:/root/.npm
        -v pg_doorman_dotnet:/root/.dotnet
        -v pg_doorman_nuget:/root/.nuget
    )

    log_info "Running: ${cmd}"
    docker run "${docker_args[@]}" "${FULL_IMAGE}" bash -c "${cmd}"
}

# Function to build pg_doorman inside container
build_doorman() {
    log_info "Building pg_doorman..."
    run_in_container "cargo build --release"
    log_info "pg_doorman built successfully"
}

# Function to run BDD tests
run_bdd_tests() {
    local tags="${1:-}"
    log_info "Running BDD tests${tags:+ with tags: ${tags}}"

    local cmd="cargo test --test bdd"
    if [ -n "$tags" ]; then
        cmd="${cmd} -- --tags ${tags}"
    fi

    run_in_container "$cmd"
}

# Function to run Go client tests
test_go() {
    log_info "Running Go client BDD tests..."
    run_in_container "cargo test --test bdd -- --tags @go"
}

# Function to run Rust client tests
test_rust() {
    log_info "Running Rust client BDD tests..."
    run_in_container "cargo test --test bdd -- --tags @rust"
}

# Function to run Python client tests
test_python() {
    log_info "Running Python client BDD tests..."
    run_in_container "cargo test --test bdd -- --tags @python"
}

# Function to run Node.js client tests
test_nodejs() {
    log_info "Running Node.js client BDD tests..."
    run_in_container "cargo test --test bdd -- --tags @nodejs"
}

# Function to run .NET client tests
test_dotnet() {
    log_info "Running .NET client BDD tests..."
    run_in_container "cargo test --test bdd -- --tags @dotnet"
}

# Function to open interactive shell
open_shell() {
    log_info "Opening interactive shell in test environment..."
    run_in_container "bash" true
}

# Function to show usage
usage() {
    cat << EOF
Usage: $0 <command> [options]

Commands:
    pull                  Pull the latest test image from registry
    shell                 Open interactive bash shell in container
    build                 Build pg_doorman inside container

    bdd [tags]           Run BDD/Cucumber tests (optionally with tags like @go, @python)
    test-go              Run Go client BDD tests
    test-rust            Run Rust client BDD tests
    test-python          Run Python client BDD tests
    test-nodejs          Run Node.js client BDD tests
    test-dotnet          Run .NET client BDD tests

    help                 Show this help message

Environment variables:
    REGISTRY             Container registry (default: ghcr.io)
    REPO                 Repository name (auto-detected from git)
    IMAGE_TAG            Image tag to use (default: latest)

Examples:
    $0 pull                    # Pull latest image
    $0 shell                   # Interactive shell
    $0 build                   # Build pg_doorman
    $0 bdd @go                 # Run BDD tests tagged with @go
    $0 test-rust               # Run Rust client tests

EOF
}

# Main command dispatcher
case "${1:-help}" in
    pull)
        pull_image
        ;;
    shell)
        try_pull_image
        open_shell
        ;;
    build)
        try_pull_image
        build_doorman
        ;;
    bdd)
        try_pull_image
        run_bdd_tests "${2:-}"
        ;;
    test-go)
        try_pull_image
        test_go
        ;;
    test-rust)
        try_pull_image
        test_rust
        ;;
    test-python)
        try_pull_image
        test_python
        ;;
    test-nodejs)
        try_pull_image
        test_nodejs
        ;;
    test-dotnet)
        try_pull_image
        test_dotnet
        ;;
    help|--help|-h)
        usage
        ;;
    *)
        log_error "Unknown command: $1"
        usage
        exit 1
        ;;
esac
