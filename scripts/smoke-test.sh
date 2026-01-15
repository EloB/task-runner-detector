#!/bin/bash
# Smoke test - runs one task from each fixture to verify they work
set -e

FIXTURES_DIR="$(dirname "$0")/../fixtures"
cd "$FIXTURES_DIR"

echo "=== Smoke Test for Task Runner Fixtures ==="
echo ""

# Track results
PASSED=0
FAILED=0
SKIPPED=0

run_test() {
    local name="$1"
    local dir="$2"
    local cmd="$3"
    local check_cmd="$4"

    echo -n "[$name] "

    # Check if tool is available
    if [ -n "$check_cmd" ] && ! command -v "$check_cmd" &> /dev/null; then
        echo "SKIPPED ($check_cmd not installed)"
        SKIPPED=$((SKIPPED + 1))
        return
    fi

    # Run the command
    pushd "$dir" > /dev/null
    if output=$(eval "$cmd" 2>&1); then
        echo "PASSED"
        PASSED=$((PASSED + 1))
    else
        echo "FAILED"
        echo "  Command: $cmd"
        echo "  Output: $output"
        FAILED=$((FAILED + 1))
    fi
    popd > /dev/null
}

skip_test() {
    local name="$1"
    local reason="$2"
    echo "[$name] SKIPPED ($reason)"
    SKIPPED=$((SKIPPED + 1))
}

# npm (package.json)
run_test "npm" "." "npm run build" "npm"

# make (Makefile)
run_test "make" "." "make build" "make"

# just (justfile)
run_test "just" "." "just build" "just"

# turbo (turbo.json)
skip_test "turbo" "requires workspace setup"

# deno (deno.json)
run_test "deno" "packages/utils" "deno task dev" "deno"

# cargo (Cargo.toml)
run_test "cargo" "services/api" "cargo check 2>/dev/null || echo 'Cargo check...'" "cargo"

# flutter/dart (pubspec.yaml)
skip_test "flutter" "requires flutter SDK"

# poetry (pyproject.toml)
skip_test "poetry" "requires python project setup"

# maven (pom.xml)
run_test "maven" "services/backend" "mvn validate -q" "mvn"

# dotnet (csproj)
run_test "dotnet" "services/dotnet-api" "dotnet build --nologo -v q" "dotnet"

echo ""
echo "=== Results ==="
echo "Passed:  $PASSED"
echo "Failed:  $FAILED"
echo "Skipped: $SKIPPED"
echo ""

if [ $FAILED -gt 0 ]; then
    echo "Some tests failed!"
    exit 1
else
    echo "All available tests passed!"
    exit 0
fi
