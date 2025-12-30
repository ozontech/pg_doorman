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

PROJECT_DIR="tests/dotnet/prj/${TEST_NAME}"

# Create project directory
mkdir -p "${PROJECT_DIR}"
cd "${PROJECT_DIR}"

# Initialize .NET project
dotnet new sln --name "${TEST_NAME}" --force
dotnet new console --output . --force
dotnet add package Npgsql

# Copy test source
cp -f "../../data/${SOURCE_FILE}" ./Program.cs

# Run the test
dotnet run
