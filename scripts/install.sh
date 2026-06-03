#!/usr/bin/env bash
# Cori installer.
#
# Downloads the `cori` binary and the pinned Temporal CLI binary from
# their GitHub Releases, verifies sha256 checksums, and installs both
# into the first writable location on `PATH` (preferring
# `/usr/local/bin`, falling back to `~/.local/bin`).
#
# Usage:
#   curl -fsSL https://cli.cori.do/install.sh | bash
#   curl -fsSL https://cli.cori.do/install.sh | bash -s -- --version v0.1.0
#   curl -fsSL https://cli.cori.do/install.sh | bash -s -- --prefix ~/bin
#
# Environment overrides:
#   CORI_VERSION       Specific Cori release tag (default: latest)
#   CORI_INSTALL_DIR   Override install directory
#   CORI_REPO          Override GitHub repo (default: cori-do/cori)
#   TEMPORAL_VERSION   Pin Temporal CLI to a specific version
set -euo pipefail

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------

CORI_REPO="${CORI_REPO:-cori-do/cori}"
CORI_VERSION="${CORI_VERSION:-latest}"
CORI_INSTALL_DIR="${CORI_INSTALL_DIR:-}"
BIN_NAME="cori"

TEMPORAL_REPO="temporalio/cli"
DEFAULT_TEMPORAL_VERSION="1.7.0"
TEMPORAL_VERSION="${TEMPORAL_VERSION:-}"

DENO_REPO="denoland/deno"
DEFAULT_DENO_VERSION="2.8.1"
DENO_VERSION="${DENO_VERSION:-}"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarn:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Linux)  os_part="unknown-linux-gnu" ;;
    Darwin) os_part="apple-darwin" ;;
    CYGWIN*|MINGW*|MSYS*) os_part="pc-windows-msvc" ;;
    *) die "unsupported OS: $os" ;;
  esac
  case "$arch" in
    x86_64|amd64)  arch_part="x86_64" ;;
    arm64|aarch64) arch_part="aarch64" ;;
    *) die "unsupported architecture: $arch" ;;
  esac
  TARGET="${arch_part}-${os_part}"
  if [[ "$os_part" == "pc-windows-msvc" ]]; then
    ARCHIVE_EXT="zip"
    BIN_FILE="${BIN_NAME}.exe"
  else
    ARCHIVE_EXT="tar.gz"
    BIN_FILE="${BIN_NAME}"
  fi
}

pick_install_dir() {
  if [[ -n "$CORI_INSTALL_DIR" ]]; then
    mkdir -p "$CORI_INSTALL_DIR"
    INSTALL_DIR="$CORI_INSTALL_DIR"
    return
  fi
  for candidate in /usr/local/bin "$HOME/.local/bin" "$HOME/bin"; do
    if { [[ -w "$candidate" ]] || mkdir -p "$candidate" 2>/dev/null; } && [[ -w "$candidate" ]]; then
      INSTALL_DIR="$candidate"
      return
    fi
  done
  die "no writable install directory found. Re-run with sudo or set CORI_INSTALL_DIR."
}

resolve_version() {
  if [[ "$CORI_VERSION" != "latest" ]]; then
    CORI_TAG="$CORI_VERSION"
    return
  fi
  log "Resolving latest Cori release tag…"

  # List releases (not /releases/latest) so we can skip non-CLI tags like
  # `desktop-vX.Y.Z` and pick the newest `vX.Y.Z[-prerelease]` tag instead.
  local api_url="https://api.github.com/repos/${CORI_REPO}/releases?per_page=100"
  local body http_code curl_args=(-sSL -w '\n%{http_code}' -H 'Accept: application/vnd.github+json')
  [[ -n "${GITHUB_TOKEN:-}" ]] && curl_args+=(-H "Authorization: Bearer ${GITHUB_TOKEN}")

  # Don't pipe curl directly — `set -o pipefail` would mask the HTTP status
  # and a transient 403/404 would exit the script silently.
  if ! body="$(curl "${curl_args[@]}" "$api_url")"; then
    die "could not reach GitHub API ($api_url). Check your network."
  fi
  http_code="${body##*$'\n'}"
  body="${body%$'\n'*}"

  if [[ "$http_code" != "200" ]]; then
    warn "GitHub API returned HTTP $http_code for $api_url"
    [[ -n "$body" ]] && printf '%s\n' "$body" >&2
    die "could not determine latest Cori release (set CORI_VERSION=vX.Y.Z to pin a tag, or export GITHUB_TOKEN if rate-limited)"
  fi

  CORI_TAG="$(printf '%s' "$body" \
    | grep '"tag_name"' \
    | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/' \
    | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+([-+].*)?$' \
    | head -n1)"
  [[ -n "$CORI_TAG" ]] || die "could not find a semver-tagged Cori release (set CORI_VERSION=vX.Y.Z to pin a tag)"
  log "Latest release: $CORI_TAG"
}

download() {
  local url="$1" dest="$2"
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
  local stem="${BIN_NAME}-${CORI_TAG}-${TARGET}"
  local archive_name="${stem}.${ARCHIVE_EXT}"
  local base_url="https://github.com/${CORI_REPO}/releases/download/${CORI_TAG}"
  local archive="$WORK_DIR/$archive_name"
  local sums="$WORK_DIR/${archive_name}.sha256"

  log "Downloading $archive_name"
  download "${base_url}/${archive_name}" "$archive"

  if download "${base_url}/${archive_name}.sha256" "$sums" 2>/dev/null; then
    local expected
    expected="$(awk '{print $1}' "$sums" | head -n1)"
    verify_sha256 "$archive" "$expected"
  else
    warn "no checksum file published for ${archive_name} — skipping verification"
  fi

  local extract_dir="$WORK_DIR/extract"
  mkdir -p "$extract_dir"
  if [[ "$ARCHIVE_EXT" == "zip" ]]; then
    command -v unzip >/dev/null || die "unzip is required to extract $archive_name"
    unzip -q "$archive" -d "$extract_dir"
  else
    tar -xzf "$archive" -C "$extract_dir"
  fi

  # Release archives stage the binary under `dist/<stem>/`; on extraction
  # the layout is either `<stem>/<bin>` (unix tar) or `<bin>` at the root
  # (Windows zip — Compress-Archive packs the contents directly).
  local found
  found="$(find "$extract_dir" -type f -name "$BIN_FILE" -print -quit)"
  [[ -n "$found" ]] || die "could not find $BIN_FILE inside $archive_name"

  install -m 0755 "$found" "$INSTALL_DIR/$BIN_FILE"
  log "Installed ${BIN_NAME} ${CORI_TAG} → $INSTALL_DIR/$BIN_FILE"
}

install_temporal() {
  if command -v temporal >/dev/null; then
    log "temporal already on PATH ($(command -v temporal)) — skipping"
    return
  fi
  local temporal_os temporal_arch
  case "$(uname -s)" in
    Linux)  temporal_os="linux" ;;
    Darwin) temporal_os="darwin" ;;
    CYGWIN*|MINGW*|MSYS*)
      warn "Temporal CLI auto-install is not supported on Windows — install it manually from https://github.com/${TEMPORAL_REPO}/releases"
      return
      ;;
    *) warn "unsupported OS for temporal install — skipping"; return ;;
  esac
  case "$(uname -m)" in
    x86_64|amd64)  temporal_arch="amd64" ;;
    arm64|aarch64) temporal_arch="arm64" ;;
    *) warn "unsupported architecture for temporal install — skipping"; return ;;
  esac

  local resolved_version="${TEMPORAL_VERSION:-$DEFAULT_TEMPORAL_VERSION}"

  local asset="temporal_cli_${resolved_version}_${temporal_os}_${temporal_arch}.tar.gz"
  local archive="$WORK_DIR/temporal.tar.gz"
  local base_url="https://github.com/${TEMPORAL_REPO}/releases/download/v${resolved_version}"
  log "Downloading temporal CLI v${resolved_version}"
  download "${base_url}/${asset}" "$archive"

  local sums="$WORK_DIR/temporal-checksums.txt"
  if download "${base_url}/checksums.txt" "$sums" 2>/dev/null; then
    local expected
    expected="$(grep "${asset}" "$sums" | awk '{print $1}')"
    if [[ -n "$expected" ]]; then
      verify_sha256 "$archive" "$expected"
    else
      warn "no entry for ${asset} in checksums.txt — skipping verification"
    fi
  else
    warn "no checksums.txt available for temporal v${resolved_version} — skipping verification"
  fi
  tar -xzf "$archive" -C "$WORK_DIR"
  install -m 0755 "$WORK_DIR/temporal" "$INSTALL_DIR/temporal"
  log "Installed temporal ${resolved_version} → $INSTALL_DIR/temporal"
}

install_deno() {
  if command -v deno >/dev/null; then
    log "deno already on PATH ($(command -v deno)) — skipping"
    return
  fi
  local deno_os deno_arch
  case "$(uname -s)" in
    Linux)  deno_os="unknown-linux-gnu" ;;
    Darwin) deno_os="apple-darwin" ;;
    CYGWIN*|MINGW*|MSYS*)
      warn "Deno auto-install is not supported on Windows — install it manually from https://deno.com/"
      return
      ;;
    *) warn "unsupported OS for deno install — skipping"; return ;;
  esac
  case "$(uname -m)" in
    x86_64|amd64)  deno_arch="x86_64" ;;
    arm64|aarch64) deno_arch="aarch64" ;;
    *) warn "unsupported architecture for deno install — skipping"; return ;;
  esac

  local resolved_version="${DENO_VERSION:-$DEFAULT_DENO_VERSION}"
  local asset="deno-${deno_arch}-${deno_os}.zip"
  local archive="$WORK_DIR/deno.zip"
  local url="https://github.com/${DENO_REPO}/releases/download/v${resolved_version}/${asset}"
  log "Downloading Deno v${resolved_version}"
  download "$url" "$archive"

  local sums="$WORK_DIR/deno.zip.sha256sum"
  if download "${url}.sha256sum" "$sums" 2>/dev/null; then
    local expected
    # Deno's Windows .sha256sum is PowerShell Get-FileHash output with
    # uppercase hex; Linux/macOS use the standard `<hash>  <file>` form.
    # Grep any 64-char hex run and lowercase it to cover both.
    expected="$(grep -oiE '[a-f0-9]{64}' "$sums" | head -n1 | tr 'A-Z' 'a-z')"
    verify_sha256 "$archive" "$expected"
  else
    warn "no checksum file available for ${asset} — skipping verification"
  fi

  command -v unzip >/dev/null || die "unzip is required to extract deno"
  unzip -q -o "$archive" -d "$WORK_DIR"
  install -m 0755 "$WORK_DIR/deno" "$INSTALL_DIR/deno"
  log "Installed deno ${resolved_version} → $INSTALL_DIR/deno"
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
    --version)          CORI_VERSION="$2"; shift 2 ;;
    --prefix|--dir)     CORI_INSTALL_DIR="$2"; shift 2 ;;
    --repo)             CORI_REPO="$2"; shift 2 ;;
    --temporal-version) TEMPORAL_VERSION="$2"; shift 2 ;;
    --deno-version)     DENO_VERSION="$2"; shift 2 ;;
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

detect_target
pick_install_dir
resolve_version

WORK_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t cori-install)"
trap 'rm -rf "$WORK_DIR"' EXIT

install_cori
install_temporal
install_deno
ensure_path

log "Done. Try:"
printf '    cori run cori-do/workflows/code_only\n'
