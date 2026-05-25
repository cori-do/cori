#!/usr/bin/env bash
# Cori installer.
#
# Downloads the `cori` binary and the pinned Temporal CLI binary from the
# matching GitHub Releases, verifies checksums, and installs both into
# the first writable location on `PATH` (preferring `/usr/local/bin/`,
# falling back to `~/.local/bin/`).
#
# Usage:
#   curl -fsSL https://cli.cori.do/install.sh | bash
#   curl -fsSL https://cli.cori.do/install.sh | bash -s -- --version 0.1.0
#   curl -fsSL https://cli.cori.do/install.sh | bash -s -- --prefix ~/bin
#
# Environment overrides:
#   CORI_VERSION       Specific Cori release tag (default: latest)
#   CORI_INSTALL_DIR   Override install directory
#   TEMPORAL_VERSION   Pin Temporal CLI to a specific version
set -euo pipefail

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------

CORI_REPO="cori-do/cori"
TEMPORAL_REPO="temporalio/cli"
# Pinned default — kept in sync with `temporal-versions.toml` in the repo.
DEFAULT_TEMPORAL_VERSION="1.1.1"

CORI_VERSION="${CORI_VERSION:-latest}"
TEMPORAL_VERSION="${TEMPORAL_VERSION:-$DEFAULT_TEMPORAL_VERSION}"
CORI_INSTALL_DIR="${CORI_INSTALL_DIR:-}"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarn:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

detect_platform() {
  local os arch
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  case "$os" in
    linux)  CORI_OS="linux"  TEMPORAL_OS="linux"  ;;
    darwin) CORI_OS="darwin" TEMPORAL_OS="darwin" ;;
    *)      die "unsupported OS: $os (Windows users: please use WSL2)" ;;
  esac
  case "$arch" in
    x86_64|amd64)  CORI_ARCH="x86_64" TEMPORAL_ARCH="amd64" ;;
    arm64|aarch64) CORI_ARCH="aarch64" TEMPORAL_ARCH="arm64" ;;
    *) die "unsupported architecture: $arch" ;;
  esac
}

pick_install_dir() {
  if [[ -n "$CORI_INSTALL_DIR" ]]; then
    mkdir -p "$CORI_INSTALL_DIR"
    INSTALL_DIR="$CORI_INSTALL_DIR"
    return
  fi
  for candidate in /usr/local/bin "$HOME/.local/bin" "$HOME/bin"; do
    if [[ -w "$candidate" ]] || mkdir -p "$candidate" 2>/dev/null && [[ -w "$candidate" ]]; then
      INSTALL_DIR="$candidate"
      return
    fi
  done
  die "no writable install directory found. Re-run with sudo or set CORI_INSTALL_DIR."
}

resolve_cori_version() {
  if [[ "$CORI_VERSION" != "latest" ]]; then
    CORI_TAG="$CORI_VERSION"
    return
  fi
  log "Resolving latest Cori release tag…"
  CORI_TAG="$(
    curl -fsSL "https://api.github.com/repos/${CORI_REPO}/releases/latest" |
      grep -m1 '"tag_name"' |
      sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'
  )"
  [[ -n "$CORI_TAG" ]] || die "could not determine latest Cori release"
}

download() {
  local url="$1" dest="$2"
  log "Downloading $(basename "$dest")…"
  if command -v curl >/dev/null; then
    curl -fL --progress-bar -o "$dest" "$url"
  elif command -v wget >/dev/null; then
    wget -q --show-progress -O "$dest" "$url"
  else
    die "need either curl or wget"
  fi
}

verify_sha256() {
  local file="$1" expected="$2"
  if [[ -z "$expected" ]]; then
    warn "no checksum supplied for $(basename "$file") — skipping verification"
    return
  fi
  local actual
  if command -v sha256sum >/dev/null; then
    actual="$(sha256sum "$file" | awk '{print $1}')"
  elif command -v shasum >/dev/null; then
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  else
    warn "no sha256sum/shasum available — skipping checksum verification"
    return
  fi
  if [[ "$actual" != "$expected" ]]; then
    die "checksum mismatch for $(basename "$file"): expected $expected, got $actual"
  fi
}

install_cori() {
  local archive="$WORK_DIR/cori.tar.gz"
  local url="https://github.com/${CORI_REPO}/releases/download/${CORI_TAG}/cori-${CORI_TAG}-${CORI_OS}-${CORI_ARCH}.tar.gz"
  download "$url" "$archive"
  # Best-effort checksum fetch (no hard failure if the release doesn't
  # ship a SHASUMS file yet — the project may be pre-1.0).
  local checksums="$WORK_DIR/cori.SHA256SUMS"
  if curl -fsSL -o "$checksums" \
      "https://github.com/${CORI_REPO}/releases/download/${CORI_TAG}/SHA256SUMS" 2>/dev/null; then
    local expected
    expected="$(grep "cori-${CORI_TAG}-${CORI_OS}-${CORI_ARCH}.tar.gz" "$checksums" | awk '{print $1}')"
    verify_sha256 "$archive" "$expected"
  fi
  tar -xzf "$archive" -C "$WORK_DIR"
  install -m 0755 "$WORK_DIR/cori" "$INSTALL_DIR/cori"
  log "Installed cori $CORI_TAG → $INSTALL_DIR/cori"
}

install_temporal() {
  if command -v temporal >/dev/null; then
    log "temporal already on PATH ($(command -v temporal)) — skipping"
    return
  fi
  local archive="$WORK_DIR/temporal.tar.gz"
  local url="https://github.com/${TEMPORAL_REPO}/releases/download/v${TEMPORAL_VERSION}/temporal_cli_${TEMPORAL_VERSION}_${TEMPORAL_OS}_${TEMPORAL_ARCH}.tar.gz"
  download "$url" "$archive"
  tar -xzf "$archive" -C "$WORK_DIR"
  install -m 0755 "$WORK_DIR/temporal" "$INSTALL_DIR/temporal"
  log "Installed temporal $TEMPORAL_VERSION → $INSTALL_DIR/temporal"
}

ensure_path() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) return ;;
  esac
  warn "$INSTALL_DIR is not on your PATH."
  warn "Add this to your shell profile:"
  printf '    export PATH="%s:$PATH"\n' "$INSTALL_DIR"
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)         CORI_VERSION="$2"; shift 2 ;;
    --prefix|--dir)    CORI_INSTALL_DIR="$2"; shift 2 ;;
    --temporal-version) TEMPORAL_VERSION="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,/^set -euo pipefail/p' "$0" | sed -n 's/^# \{0,1\}//p' | sed '$d'
      exit 0
      ;;
    *) die "unknown flag: $1" ;;
  esac
done

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

detect_platform
pick_install_dir
resolve_cori_version

WORK_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t cori-install)"
trap 'rm -rf "$WORK_DIR"' EXIT

install_cori
install_temporal
ensure_path

log "Done. Try:"
printf '    cori init --local\n'
printf '    cori demo\n'
printf '    cori skill install --agent claude-code\n'
