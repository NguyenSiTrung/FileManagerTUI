#!/usr/bin/env bash
# check-terminal-workspace.sh — terminal-workspace quality runner.
#
# Executes every configured gate and exits nonzero when a MANDATORY gate
# fails or its harness is missing. Optional evidence steps (coverage,
# static-musl build) run when their tooling is installed and report a loud
# SKIP otherwise — never a silent pass.
#
# Usage:
#   scripts/check-terminal-workspace.sh           # all gates
#   scripts/check-terminal-workspace.sh --quick   # rust gates only
set -u -o pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"

QUICK=0
[ "${1:-}" = "--quick" ] && QUICK=1

FAILURES=0
ran=()
fail=()

gate() {
  local name="$1"; shift
  printf '\n===== %s =====\n' "$name"
  if "$@"; then
    ran+=("PASS  $name")
  else
    ran+=("FAIL  $name"); fail+=("$name"); FAILURES=$((FAILURES + 1))
  fi
}

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    ran+=("BLOCK $name: missing harness '$2'")
    fail+=("$name (missing $2)"); FAILURES=$((FAILURES + 1))
    return 1
  fi
  return 0
}

# ── Rust gates (mandatory) ──────────────────────────────────────────────────
gate "cargo fmt --check" cargo fmt --check
gate "cargo clippy" cargo clippy -- -D warnings
gate "cargo clippy --all-targets" cargo clippy --all-targets -- -D warnings
gate "cargo test" cargo test
gate "cargo build --release" cargo build --release

# ── Binary size evidence (report; >10 MiB fails — the documented NFR cap) ───
FM_BIN="$ROOT/target/release/fm"
if [ -x "$FM_BIN" ]; then
  size_bytes=$(stat -c %s "$FM_BIN")
  size_mib=$((size_bytes / 1048576))
  echo "release binary size: ${size_bytes} bytes (${size_mib} MiB)"
  if [ "$size_bytes" -gt 10485760 ]; then
    ran+=("FAIL  binary size ${size_mib} MiB > 10 MiB")
    fail+=("binary size"); FAILURES=$((FAILURES + 1))
  else
    ran+=("PASS  binary size ${size_mib} MiB <= 10 MiB")
  fi
else
  ran+=("FAIL  release binary missing at $FM_BIN")
  fail+=("release binary"); FAILURES=$((FAILURES + 1))
fi

if [ "$QUICK" -eq 0 ]; then
  # ── PTY scenarios (mandatory) ─────────────────────────────────────────────
  if command -v python3 >/dev/null 2>&1; then
    gate "PTY acceptance matrix" python3 scripts/test-terminal-workspace.py
  else
    ran+=("BLOCK PTY acceptance matrix: missing harness 'python3'")
    fail+=("PTY (missing python3)"); FAILURES=$((FAILURES + 1))
  fi

  # ── Browser-terminal suite (mandatory) ────────────────────────────────────
  if command -v npm >/dev/null 2>&1 && command -v node >/dev/null 2>&1; then
    if [ ! -d tools/terminal-tests/node_modules ]; then
      echo "installing terminal-tests deps (npm ci)"
      npm --prefix tools/terminal-tests ci || {
        ran+=("BLOCK browser suite: npm ci failed")
        fail+=("browser npm ci"); FAILURES=$((FAILURES + 1))
      }
    fi
    if [ $FAILURES -eq 0 ] || [ -d tools/terminal-tests/node_modules ]; then
      gate "browser-terminal suite" npm --prefix tools/terminal-tests test
    fi
  else
    ran+=("BLOCK browser suite: missing harness 'node/npm'")
    fail+=("browser (missing node/npm)"); FAILURES=$((FAILURES + 1))
  fi

  # ── Coverage evidence (optional tooling) ──────────────────────────────────
  if command -v cargo-llvm-cov >/dev/null 2>&1 || cargo llvm-cov --version >/dev/null 2>&1; then
    gate "coverage summary (llvm-cov)" \
      cargo llvm-cov --offline --all-features --summary-only
  else
    ran+=("SKIP  coverage: cargo-llvm-cov not installed (optional evidence)")
  fi

  # ── Static-musl build check (optional tooling) ────────────────────────────
  if rustup target list --installed 2>/dev/null | grep -q '^x86_64-unknown-linux-musl$'; then
    gate "static-musl release build" \
      cargo build --release --target x86_64-unknown-linux-musl
    musl_bin="$ROOT/target/x86_64-unknown-linux-musl/release/fm"
    [ -x "$musl_bin" ] && \
      echo "musl binary size: $(stat -c %s "$musl_bin") bytes"
  else
    ran+=("SKIP  musl build: target not installed (rustup target add x86_64-unknown-linux-musl)")
  fi
fi

# ── Summary ─────────────────────────────────────────────────────────────────
printf '\n================ gate summary ================\n'
for line in "${ran[@]}"; do echo "  $line"; done
if [ "$FAILURES" -gt 0 ]; then
  printf '\n%d mandatory gate(s) failed/blocked: %s\n' "$FAILURES" "${fail[*]}"
  exit 1
fi
echo "all mandatory gates passed"
exit 0
