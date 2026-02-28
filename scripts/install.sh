#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# fm-tui installer — installs Rust/Cargo via rustup and builds fm-tui
# Usage:  curl -fsSL <raw-url>/scripts/install.sh | bash
#     or: ./scripts/install.sh
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

# ── Colors ───────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

info()    { printf "${CYAN}[INFO]${RESET}  %s\n" "$*"; }
success() { printf "${GREEN}[  OK]${RESET}  %s\n" "$*"; }
warn()    { printf "${YELLOW}[WARN]${RESET}  %s\n" "$*"; }
error()   { printf "${RED}[FAIL]${RESET}  %s\n" "$*" >&2; }

# ── Helpers ──────────────────────────────────────────────────────────────────
command_exists() { command -v "$1" &>/dev/null; }

ensure_dependencies() {
    local missing=()
    for cmd in curl gcc make; do
        command_exists "$cmd" || missing+=("$cmd")
    done

    if [[ ${#missing[@]} -gt 0 ]]; then
        warn "Missing system dependencies: ${missing[*]}"
        info "Attempting to install them..."

        if command_exists apt-get; then
            sudo apt-get update -qq
            sudo apt-get install -y -qq build-essential curl pkg-config libssl-dev
        elif command_exists dnf; then
            sudo dnf install -y gcc make curl openssl-devel pkg-config
        elif command_exists pacman; then
            sudo pacman -Sy --noconfirm base-devel curl openssl pkg-config
        elif command_exists brew; then
            brew install curl openssl pkg-config
        else
            error "Could not auto-install dependencies. Please install: ${missing[*]}"
            exit 1
        fi
        success "System dependencies installed"
    fi
}

# ── Step 1 — System deps ────────────────────────────────────────────────────
printf "\n${BOLD}══════════════════════════════════════════════════${RESET}\n"
printf "${BOLD}  fm-tui — Install & Setup${RESET}\n"
printf "${BOLD}══════════════════════════════════════════════════${RESET}\n\n"

info "Checking system dependencies..."
ensure_dependencies

# ── Step 2 — Rust & Cargo ───────────────────────────────────────────────────
if command_exists rustc && command_exists cargo; then
    RUST_VER=$(rustc --version)
    CARGO_VER=$(cargo --version)
    success "Rust already installed: ${RUST_VER}"
    success "Cargo already installed: ${CARGO_VER}"

    # Check for updates
    info "Checking for Rust updates..."
    if command_exists rustup; then
        rustup update stable 2>/dev/null || true
        success "Rust toolchain is up to date"
    fi
else
    info "Installing Rust via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable

    # Source cargo env for current session
    if [[ -f "$HOME/.cargo/env" ]]; then
        # shellcheck source=/dev/null
        source "$HOME/.cargo/env"
    elif [[ -f "$HOME/.rustup/env" ]]; then
        # shellcheck source=/dev/null
        source "$HOME/.rustup/env"
    fi

    if command_exists rustc && command_exists cargo; then
        success "Rust installed: $(rustc --version)"
        success "Cargo installed: $(cargo --version)"
    else
        error "Rust installation failed. Please install manually: https://rustup.rs"
        exit 1
    fi
fi

# ── Step 3 — Install useful Cargo components ────────────────────────────────
info "Ensuring Cargo components are installed..."

# clippy & rustfmt (for development)
rustup component add clippy 2>/dev/null && success "clippy ready" || warn "clippy already installed"
rustup component add rustfmt 2>/dev/null && success "rustfmt ready" || warn "rustfmt already installed"

# ── Step 4 — Build fm-tui ───────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

if [[ -f "$PROJECT_DIR/Cargo.toml" ]]; then
    info "Building fm-tui in release mode..."
    cd "$PROJECT_DIR"
    cargo build --release

    BINARY="$PROJECT_DIR/target/release/fm"
    if [[ -f "$BINARY" ]]; then
        success "Build complete: $BINARY"

        printf "\n${BOLD}── Install binary? ──${RESET}\n"
        printf "  1) Copy to ${CYAN}/usr/local/bin/fm${RESET} (requires sudo)\n"
        printf "  2) Copy to ${CYAN}~/.cargo/bin/fm${RESET}\n"
        printf "  3) Skip installation\n"
        printf "\n"

        read -rp "Choose [1/2/3] (default: 2): " choice
        choice="${choice:-2}"

        case "$choice" in
            1)
                sudo cp "$BINARY" /usr/local/bin/fm
                sudo chmod +x /usr/local/bin/fm
                success "Installed to /usr/local/bin/fm"
                ;;
            2)
                mkdir -p "$HOME/.cargo/bin"
                cp "$BINARY" "$HOME/.cargo/bin/fm"
                chmod +x "$HOME/.cargo/bin/fm"
                success "Installed to ~/.cargo/bin/fm"
                ;;
            3)
                info "Skipped. Binary is at: $BINARY"
                ;;
            *)
                warn "Invalid choice. Skipped."
                info "Binary is at: $BINARY"
                ;;
        esac
    else
        error "Build succeeded but binary not found at expected path"
        exit 1
    fi
else
    warn "Cargo.toml not found — skipping build step"
    info "To install fm-tui directly:"
    printf "  ${CYAN}cargo install --git https://github.com/NguyenSiTrung/FileManagerTUI.git${RESET}\n"
fi

# ── Step 5 — Verify ─────────────────────────────────────────────────────────
printf "\n${BOLD}══════════════════════════════════════════════════${RESET}\n"
printf "${BOLD}  Summary${RESET}\n"
printf "${BOLD}══════════════════════════════════════════════════${RESET}\n\n"

printf "  %-14s %s\n" "Rust:" "$(rustc --version 2>/dev/null || echo 'not found')"
printf "  %-14s %s\n" "Cargo:" "$(cargo --version 2>/dev/null || echo 'not found')"
printf "  %-14s %s\n" "Clippy:" "$(cargo clippy --version 2>/dev/null || echo 'not found')"
printf "  %-14s %s\n" "Rustfmt:" "$(cargo fmt --version 2>/dev/null || echo 'not found')"

if command_exists fm; then
    printf "  %-14s %s\n" "fm-tui:" "$(which fm)"
fi

printf "\n${GREEN}${BOLD}✓ All done!${RESET} Run ${CYAN}fm${RESET} to launch the file manager.\n\n"

# Remind about PATH if cargo/bin isn't in PATH
if [[ ":$PATH:" != *":$HOME/.cargo/bin:"* ]]; then
    warn "~/.cargo/bin is not in your PATH"
    info "Add this to your shell profile (~/.bashrc or ~/.zshrc):"
    printf "\n  ${CYAN}source \"\$HOME/.cargo/env\"${RESET}\n\n"
fi
