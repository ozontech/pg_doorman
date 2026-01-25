#!/usr/bin/env bash
set -e

# Usage: ./run_test.sh <test_name> <source_file>
# Example: ./run_test.sh pbde PBDE_PBDE_S.cs

TEST_NAME=$1
SOURCE_FILE=$2

if [ -z "$TEST_NAME" ] || [ -z "$SOURCE_FILE" ]; then
    echo "Usage: $0 <test_name> <source_file>"
    exit 1
fi

# Get absolute path to data directory before changing directories
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_DIR="${SCRIPT_DIR}/data"

# Create temporary project directory
PROJECT_DIR=$(mktemp -d)
trap "rm -rf '${PROJECT_DIR}'" EXIT

cd "${PROJECT_DIR}"

# Initialize .NET project
dotnet new sln --name "${TEST_NAME}" --force
dotnet new console --output . --force
dotnet add package Npgsql

# Copy test source
cp -f "${DATA_DIR}/${SOURCE_FILE}" ./Program.cs

# Run the test
dotnet run
