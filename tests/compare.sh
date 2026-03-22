#!/usr/bin/env bash
# compare.sh — Run both TS and Rust todoapp examples with identical inputs
# and diff their JSON output. Exits non-zero if any test case differs.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUST_BIN="$ROOT/target/debug/examples/todoapp"
TS_CMD="npx tsx $ROOT/examples/todoapp.ts"

PASSED=0
FAILED=0
ERRORS=()

# Colors (disabled if not a terminal)
if [ -t 1 ]; then
  GREEN='\033[0;32m'
  RED='\033[0;31m'
  YELLOW='\033[0;33m'
  BOLD='\033[1m'
  RESET='\033[0m'
else
  GREEN=''
  RED=''
  YELLOW=''
  BOLD=''
  RESET=''
fi

# ---------------------------------------------------------------------------
# Build Rust example first
# ---------------------------------------------------------------------------
echo -e "${BOLD}Building Rust todoapp example...${RESET}"
cargo build --example todoapp -p incur --manifest-path "$ROOT/Cargo.toml" 2>&1
if [ ! -f "$RUST_BIN" ]; then
  echo -e "${RED}ERROR: Rust binary not found at $RUST_BIN${RESET}"
  exit 1
fi
echo -e "${GREEN}Build succeeded.${RESET}"
echo ""

# ---------------------------------------------------------------------------
# Helper: run a single comparison test
# ---------------------------------------------------------------------------
compare() {
  local label="$1"
  shift
  local args=("$@")

  local tmp_ts tmp_rs
  tmp_ts=$(mktemp)
  tmp_rs=$(mktemp)
  trap "rm -f '$tmp_ts' '$tmp_rs'" RETURN

  # Run TS version (stderr to /dev/null to suppress middleware logging)
  $TS_CMD "${args[@]}" > "$tmp_ts" 2>/dev/null || true

  # Run Rust version
  "$RUST_BIN" "${args[@]}" > "$tmp_rs" 2>/dev/null || true

  if diff -u "$tmp_ts" "$tmp_rs" > /dev/null 2>&1; then
    echo -e "  ${GREEN}PASS${RESET}  $label"
    PASSED=$((PASSED + 1))
  else
    echo -e "  ${RED}FAIL${RESET}  $label"
    echo -e "${YELLOW}--- TS output ---${RESET}"
    cat "$tmp_ts"
    echo -e "${YELLOW}--- Rust output ---${RESET}"
    cat "$tmp_rs"
    echo -e "${YELLOW}--- diff ---${RESET}"
    diff -u "$tmp_ts" "$tmp_rs" || true
    echo ""
    FAILED=$((FAILED + 1))
    ERRORS+=("$label")
  fi

  rm -f "$tmp_ts" "$tmp_rs"
}

# ---------------------------------------------------------------------------
# Helper: compare streaming output (collect all lines, then diff)
# ---------------------------------------------------------------------------
compare_stream() {
  local label="$1"
  shift
  local args=("$@")

  local tmp_ts tmp_rs
  tmp_ts=$(mktemp)
  tmp_rs=$(mktemp)
  trap "rm -f '$tmp_ts' '$tmp_rs'" RETURN

  # Run TS version — streams complete after all yields
  $TS_CMD "${args[@]}" > "$tmp_ts" 2>/dev/null || true

  # Run Rust version
  "$RUST_BIN" "${args[@]}" > "$tmp_rs" 2>/dev/null || true

  if diff -u "$tmp_ts" "$tmp_rs" > /dev/null 2>&1; then
    echo -e "  ${GREEN}PASS${RESET}  $label"
    PASSED=$((PASSED + 1))
  else
    echo -e "  ${RED}FAIL${RESET}  $label"
    echo -e "${YELLOW}--- TS output ---${RESET}"
    cat "$tmp_ts"
    echo -e "${YELLOW}--- Rust output ---${RESET}"
    cat "$tmp_rs"
    echo -e "${YELLOW}--- diff ---${RESET}"
    diff -u "$tmp_ts" "$tmp_rs" || true
    echo ""
    FAILED=$((FAILED + 1))
    ERRORS+=("$label")
  fi

  rm -f "$tmp_ts" "$tmp_rs"
}

# ---------------------------------------------------------------------------
# Test cases
# ---------------------------------------------------------------------------
echo -e "${BOLD}Running comparison tests...${RESET}"
echo ""

# Help output differs intentionally (Rust adds table|csv to --format).
# Compare structure only (collapse whitespace).
compare_normalized() {
  local label="$1"
  shift
  local args=("$@")
  local tmp_ts tmp_rs
  tmp_ts=$(mktemp)
  tmp_rs=$(mktemp)
  $TS_CMD "${args[@]}" 2>/dev/null | sed 's/  */ /g' > "$tmp_ts" || true
  "$RUST_BIN" "${args[@]}" 2>/dev/null | sed 's/  */ /g' > "$tmp_rs" || true
  # Compare non-format lines (skip the --format line which intentionally differs)
  local ts_filtered rs_filtered
  ts_filtered=$(grep -v "^.*--format" "$tmp_ts")
  rs_filtered=$(grep -v "^.*--format" "$tmp_rs")
  if [ "$ts_filtered" = "$rs_filtered" ]; then
    echo -e "  ${GREEN}PASS${RESET}  $label (normalized)"
    PASSED=$((PASSED + 1))
  else
    echo -e "  ${RED}FAIL${RESET}  $label"
    diff -u "$tmp_ts" "$tmp_rs" || true
    FAILED=$((FAILED + 1))
    ERRORS+=("$label")
  fi
  rm -f "$tmp_ts" "$tmp_rs"
}
compare_normalized "help"          --help
compare "version"                  --version
compare "list --json"              list --json
compare "list --status pending"    list --status pending --json
compare "get 1 --json"             get 1 --json
compare "get 99 --json (error)"    get 99 --json
compare "complete 1 --json"        complete 1 --json
compare "stats --json"             stats --json
compare "add test item --json"     add "test item" --priority high --json
compare_stream "stream"            stream --json

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo -e "${BOLD}Results: ${GREEN}$PASSED passed${RESET}, ${RED}$FAILED failed${RESET} (out of $((PASSED + FAILED)) tests)"

if [ "$FAILED" -gt 0 ]; then
  echo -e "${RED}Failed tests:${RESET}"
  for e in "${ERRORS[@]}"; do
    echo "  - $e"
  done
  exit 1
fi

echo -e "${GREEN}All tests passed!${RESET}"
exit 0
