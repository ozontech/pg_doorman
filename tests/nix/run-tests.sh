#!/usr/bin/env bash
set -e

# Configuration
REGISTRY="${REGISTRY:-ghcr.io}"
REPO="${REPO:-$(git config --get remote.origin.url | sed 's/.*://;s/.git$//')}"
IMAGE_NAME="${REGISTRY}/${REPO}/test-runner"
IMAGE_TAG="${IMAGE_TAG:-latest}"
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

# Get project root (two levels up from tests/nix)
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

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
    run_in_container "setup-test-deps && cargo build --release"
    log_info "pg_doorman built successfully"
}

# Function to run BDD tests
run_bdd_tests() {
    local tags="${1:-}"
    log_info "Running BDD tests${tags:+ with tags: ${tags}}"

    local cmd="setup-test-deps && cargo test --test bdd"
    if [ -n "$tags" ]; then
        cmd="${cmd} -- --tags ${tags}"
    fi

    run_in_container "$cmd"
}

# Function to run language-specific tests
run_go_tests() {
    log_info "Running Go tests..."
    run_in_container "cd tests/go && setup-test-deps && go test -v ."
}

run_python_tests() {
    log_info "Running Python tests..."
    run_in_container "cd tests/python && setup-test-deps && pytest -v ."
}

run_nodejs_tests() {
    log_info "Running Node.js tests..."
    run_in_container "cd tests/nodejs && setup-test-deps && npm test"
}

run_dotnet_tests() {
    log_info "Running .NET tests..."
    run_in_container "cd tests/dotnet && setup-test-deps && dotnet test"
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
    test-go              Run Go tests
    test-python          Run Python tests
    test-nodejs          Run Node.js tests
    test-dotnet          Run .NET tests
    test-all             Run all language tests

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
    $0 test-python             # Run Python tests
    $0 test-all                # Run all tests

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
        run_go_tests
        ;;
    test-python)
        try_pull_image
        run_python_tests
        ;;
    test-nodejs)
        try_pull_image
        run_nodejs_tests
        ;;
    test-dotnet)
        try_pull_image
        run_dotnet_tests
        ;;
    test-all)
        try_pull_image
        log_info "Running all language tests..."
        run_go_tests
        run_python_tests
        run_nodejs_tests
        run_dotnet_tests
        log_info "All tests completed!"
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
