#!/usr/bin/env bash
# install.sh — build & install wkhtml-rs as a drop-in `wkhtmltopdf` for Odoo.
#
# Usage:
#   sudo ./install.sh                 # system-wide (/usr/local/bin)
#   ./install.sh --user               # user-local  (~/.local/bin)
#   sudo PREFIX=/opt/wkrs ./install.sh
#
# Flags:
#   --user        install to ~/.local/bin instead of /usr/local/bin
#   --prefix DIR  install to DIR/bin (default /usr/local)
#   --force       overwrite an existing real wkhtmltopdf without prompting
set -euo pipefail

PREFIX="/usr/local"
INSTALL_USER=0
FORCE=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --user)   INSTALL_USER=1; shift ;;
        --prefix) PREFIX="$2"; shift 2 ;;
        --force)  FORCE=1; shift ;;
        -h|--help)
            sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

if [[ $INSTALL_USER -eq 1 ]]; then
    PREFIX="${HOME}/.local"
fi
BIN_DIR="${PREFIX}/bin"

say()  { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# --- source bootstrap --------------------------------------------------------
# The script can be run three ways:
#   1. from a clone:            ./install.sh            (build in place)
#   2. via curl | bash:         no repo anywhere        (clone into WORKROOT)
#   3. curl | bash inside an existing clone's dir — same as 1.
WKRS_REPO="${WKRS_REPO:-https://github.com/bastkor44/wkhmtl-rs.git}"
FULGUR_REPO="${FULGUR_REPO:-https://github.com/bastkor44/fulgur.git}"
WORKROOT="${WORKROOT:-${HOME}/.cache/wkhtml-rs}"

SCRIPT_DIR="$(dirname "$0")"
# Piped scripts live in /dev/stdin or /tmp — detect "no real checkout".
if [[ -f Cargo.toml && -d src ]]; then
    SRC_DIR="$(cd "$SCRIPT_DIR" && pwd)"
else
    SRC_DIR="$WORKROOT/src/wkhtml-rs"
fi
FULGUR_DIR="$(dirname "$SRC_DIR")/fulgur"

bootstrap() {
    local url=$1 dest=$2 name=$3
    if [[ -d $dest/.git ]]; then
        say "updating ${name} at ${dest}"
        git -C "$dest" pull --ff-only >/dev/null 2>&1 || warn "git pull failed for ${name}; using existing checkout"
    else
        say "cloning ${name} into ${dest}"
        mkdir -p "$(dirname "$dest")"
        git clone --depth 1 "$url" "$dest" >&2 || die "failed to clone ${name} from ${url}"
    fi
}

if [[ ! -f "${SRC_DIR}/Cargo.toml" ]]; then
    bootstrap "$WKRS_REPO" "$SRC_DIR" wkhtml-rs
fi
# fulgur must sit next to wkhtml-rs (Cargo relative path dep ../fulgur/...).
bootstrap "$FULGUR_REPO" "$FULGUR_DIR" fulgur

cd "$SRC_DIR"

# --- prerequisites ----------------------------------------------------------
if ! command -v cargo >/dev/null 2>&1; then
    say "cargo not found — installing Rust toolchain via rustup"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    export PATH="${HOME}/.cargo/bin:${PATH}"
fi
command -v cargo >/dev/null 2>&1 || die "cargo is still unavailable; install Rust from https://rustup.rs"

[[ -d ${FULGUR_DIR}/crates/fulgur ]] || die "fulgur crate missing at ${FULGUR_DIR}/crates/fulgur after bootstrap"

# --- existing installation --------------------------------------------------
TARGET="${BIN_DIR}/wkhtmltopdf"
if [[ -e "$TARGET" && $FORCE -eq 0 ]]; then
    warn "an existing wkhtmltopdf was found at ${TARGET}"
    if command -v wkhtmltopdf >/dev/null 2>&1 && wkhtmltopdf --version 2>/dev/null | grep -qv 'wkhtml-rs\|wkhtml-rs marker'; then
        warn "a REAL wkhtmltopdf appears to be installed — it will shadow or be shadowed by this one."
    fi
    read -r -p "Overwrite? [y/N] " reply
    [[ "${reply,,}" == y* ]] || die "aborted (use --force to skip this prompt)"
fi

# --- build -------------------------------------------------------------------
say "building release binary (this can take a few minutes)"
cargo build --release
[[ -x target/release/wkhtmltopdf ]] || die "build did not produce target/release/wkhtmltopdf"

# --- install -----------------------------------------------------------------
say "installing to ${TARGET}"
mkdir -p "$BIN_DIR"
install -m 0755 target/release/wkhtmltopdf "$TARGET"

# --- verify ------------------------------------------------------------------
VERSION_OUT="$("$TARGET" --version)"
say "installed: ${TARGET} → ${VERSION_OUT}"

echo '<html><body><h1>wkhtml-rs install check</h1></body></html>' > "/tmp/wkrs-check-$$.html"
if "$TARGET" "/tmp/wkrs-check-$$.html" "/tmp/wkrs-check-$$.pdf" >/dev/null 2>&1 && [[ -s "/tmp/wkrs-check-$$.pdf" ]]; then
    say "smoke test passed: PDF rendered successfully"
else
    warn "smoke test failed — the binary installed but rendering produced no output"
fi
rm -f "/tmp/wkrs-check-$$.html" "/tmp/wkrs-check-$$.pdf"

cat <<EOF

Done! To point Odoo at it, add to odoo.conf:

    [options]
    wkhtmltopdf = ${TARGET}

…or start Odoo with:  odoo --wkhtmltopdf=${TARGET}

Note: if \${BIN_DIR} is not on your PATH, Odoo's default lookup will miss it —
always set the explicit path above.
EOF
