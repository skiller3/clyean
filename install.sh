#!/bin/sh
set -e

# Clyean Installer
# Usage: curl -fsSL https://clyean.com/install.sh | sh
#
# With options (everything after `sh -s --` is passed to the script):
#   curl -fsSL https://clyean.com/install.sh | sh -s -- --binary
#   curl -fsSL https://clyean.com/install.sh | sh -s -- --source
#   curl -fsSL https://clyean.com/install.sh | sh -s -- --ref v0.2.0
#   curl -fsSL https://clyean.com/install.sh | sh -s -- --no-deps
#
# Options:
#   --binary       Install the prebuilt clyean binary from GitHub releases (default)
#   --source       Build and install clyean from source with cargo (requires a Rust toolchain)
#   --ref <tag>    Install a specific release tag, for example v0.2.0 (default: latest release)
#   -r <tag>       Shorthand for --ref
#   --no-deps      Do not install missing host dependencies (git, curl, podman); print instructions instead
#
# Environment:
#   CLYEAN_INSTALL_DIR   Directory that receives the clyean executable (default: $HOME/.local/bin)
#
# Clyean runs every agent inside a Podman container, so Podman is a required
# host dependency alongside git and curl. The installer never prompts: package
# managers are invoked in their non-interactive modes, and sudo is used only
# when the current user is not root.

REPO="skiller3/clyean"
INSTALL_DIR="${CLYEAN_INSTALL_DIR:-$HOME/.local/bin}"
GITHUB_API="https://api.github.com/repos/${REPO}"
GITHUB_DOWNLOAD="https://github.com/${REPO}/releases/download"

# Parse arguments
MODE=""
REF=""
INSTALL_DEPS="yes"
while [ $# -gt 0 ]; do
    case "$1" in
        --source)
            MODE="source"
            shift
            ;;
        --binary)
            MODE="binary"
            shift
            ;;
        --ref)
            shift
            if [ -z "$1" ]; then
                echo "Missing value for --ref"
                exit 1
            fi
            REF="$1"
            shift
            ;;
        --ref=*)
            REF="${1#*=}"
            if [ -z "$REF" ]; then
                echo "Missing value for --ref"
                exit 1
            fi
            shift
            ;;
        -r)
            shift
            if [ -z "$1" ]; then
                echo "Missing value for -r"
                exit 1
            fi
            REF="$1"
            shift
            ;;
        --no-deps)
            INSTALL_DEPS="no"
            shift
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

if [ -z "$MODE" ]; then
    MODE="binary"
fi

has_command() {
    command -v "$1" >/dev/null 2>&1
}

# Print a command before running it so the user always sees what the
# installer does to the host.
run_logged() {
    echo "+ $*"
    "$@"
}

# Normalized host architecture (x64|arm64). On macOS this uses
# `sysctl hw.optional.arm64` so it stays correct inside a Rosetta session,
# where `uname -m` reports the translated x86_64.
host_arch() {
    if [ "$(uname -s)" = "Darwin" ]; then
        if [ "$(sysctl -in hw.optional.arm64 2>/dev/null || /usr/sbin/sysctl -in hw.optional.arm64 2>/dev/null)" = "1" ]; then
            echo "arm64"
        else
            echo "x64"
        fi
        return
    fi
    case "$(uname -m)" in
        x86_64|amd64)  echo "x64" ;;
        arm64|aarch64) echo "arm64" ;;
        *)             uname -m ;;
    esac
}

host_platform() {
    case "$(uname -s)" in
        Linux)  echo "linux" ;;
        Darwin) echo "darwin" ;;
        *)      echo "Unsupported OS: $(uname -s)"; exit 1 ;;
    esac
}

# ---------------------------------------------------------------------------
# Host dependencies: git, curl, podman
# ---------------------------------------------------------------------------

# Prefix for commands that need root. Empty when already root.
privilege_prefix() {
    if [ "$(id -u)" = "0" ]; then
        echo ""
    elif has_command sudo; then
        echo "sudo"
    else
        echo "__missing__"
    fi
}

missing_dependencies() {
    missing=""
    for dep in git curl podman; do
        if ! has_command "$dep"; then
            missing="$missing $dep"
        fi
    done
    echo "$missing"
}

print_dependency_instructions() {
    echo ""
    echo "Install the missing dependencies, then re-run this installer:"
    case "$(host_platform)" in
        darwin)
            echo "    brew install$1"
            echo "    podman machine init && podman machine start"
            ;;
        linux)
            echo "    Debian/Ubuntu:  sudo apt-get install -y$1"
            echo "    Fedora:         sudo dnf install -y$1"
            echo "    openSUSE:       sudo zypper --non-interactive install$1"
            echo "    Arch:           sudo pacman -Sy --noconfirm$1"
            echo "    Alpine:         sudo apk add$1"
            echo "Podman documentation: https://podman.io/docs/installation"
            ;;
    esac
}

install_linux_packages() {
    packages="$1"
    prefix="$(privilege_prefix)"
    if [ "$prefix" = "__missing__" ]; then
        echo "Dependencies are missing and neither root privileges nor sudo are available."
        print_dependency_instructions "$packages"
        exit 1
    fi
    # shellcheck disable=SC2086
    if has_command apt-get; then
        run_logged $prefix apt-get update
        run_logged $prefix apt-get install -y $packages
    elif has_command dnf; then
        run_logged $prefix dnf install -y $packages
    elif has_command yum; then
        run_logged $prefix yum install -y $packages
    elif has_command zypper; then
        run_logged $prefix zypper --non-interactive install $packages
    elif has_command pacman; then
        run_logged $prefix pacman -Sy --noconfirm $packages
    elif has_command apk; then
        run_logged $prefix apk add $packages
    else
        echo "No supported package manager found (apt-get, dnf, yum, zypper, pacman, apk)."
        print_dependency_instructions "$packages"
        exit 1
    fi
}

install_macos_packages() {
    packages="$1"
    if ! has_command brew; then
        echo "Homebrew is required to install dependencies on macOS: https://brew.sh"
        print_dependency_instructions "$packages"
        exit 1
    fi
    # shellcheck disable=SC2086
    run_logged brew install $packages
}

# Podman on macOS runs containers inside a Linux virtual machine that must be
# created once and started before Clyean can launch agents.
ensure_podman_machine() {
    if [ "$(host_platform)" != "darwin" ]; then
        return 0
    fi
    machines="$(podman machine list --format '{{.Name}}' 2>/dev/null || true)"
    if [ -z "$machines" ]; then
        run_logged podman machine init
    fi
    if ! podman machine inspect --format '{{.State}}' 2>/dev/null | grep -q running; then
        run_logged podman machine start || echo "Could not start the Podman machine; run 'podman machine start' before using clyean."
    fi
}

# Rootless Podman on Linux needs subordinate UID and GID ranges for the user.
# This is a hint rather than an action because the change touches system files.
check_rootless_podman() {
    if [ "$(host_platform)" != "linux" ] || [ "$(id -u)" = "0" ]; then
        return 0
    fi
    user_name="$(id -un)"
    if ! grep -q "^${user_name}:" /etc/subuid 2>/dev/null || ! grep -q "^${user_name}:" /etc/subgid 2>/dev/null; then
        echo ""
        echo "Note: ${user_name} has no subordinate UID/GID ranges, which rootless Podman needs."
        echo "Add them with:  sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 ${user_name}"
        echo "then run:       podman system migrate"
    fi
}

ensure_dependencies() {
    missing="$(missing_dependencies)"
    if [ -n "$missing" ]; then
        if [ "$INSTALL_DEPS" = "no" ]; then
            echo "Missing dependencies:$missing (--no-deps given, not installing)."
            print_dependency_instructions "$missing"
            exit 1
        fi
        echo "Installing missing dependencies:$missing"
        case "$(host_platform)" in
            linux)  install_linux_packages "$missing" ;;
            darwin) install_macos_packages "$missing" ;;
        esac
        still_missing="$(missing_dependencies)"
        if [ -n "$still_missing" ]; then
            echo "Dependencies still missing after installation:$still_missing"
            print_dependency_instructions "$still_missing"
            exit 1
        fi
    fi
    if [ "$INSTALL_DEPS" = "yes" ]; then
        ensure_podman_machine
    fi
    check_rootless_podman
}

# ---------------------------------------------------------------------------
# Release lookup
# ---------------------------------------------------------------------------

release_tag() {
    if [ -n "$REF" ]; then
        echo "Fetching release $REF..." >&2
        if RELEASE_JSON=$(curl -fsSL --connect-timeout 10 --max-time 60 "${GITHUB_API}/releases/tags/${REF}"); then
            :
        else
            echo "Release tag not found: $REF" >&2
            exit 1
        fi
    else
        echo "Fetching latest release..." >&2
        RELEASE_JSON=$(curl -fsSL --connect-timeout 10 --max-time 60 "${GITHUB_API}/releases/latest")
    fi
    tag=$(echo "$RELEASE_JSON" | grep '"tag_name"' | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')
    if [ -z "$tag" ]; then
        echo "Failed to fetch release tag" >&2
        exit 1
    fi
    echo "$tag"
}

# ---------------------------------------------------------------------------
# Source install (cargo)
# ---------------------------------------------------------------------------

install_via_cargo() {
    if ! has_command cargo; then
        echo "cargo is required for --source. Install a Rust toolchain from https://rustup.rs and re-run."
        exit 1
    fi
    TAG="$(release_tag)"
    echo "Building clyean $TAG from source..."
    TMP_ROOT="$(mktemp -d)"
    trap 'rm -rf "$TMP_ROOT"' EXIT
    run_logged cargo install --locked --git "https://github.com/${REPO}" --tag "$TAG" --root "$TMP_ROOT" clyean
    mkdir -p "$INSTALL_DIR"
    cp "$TMP_ROOT/bin/clyean" "$INSTALL_DIR/clyean"
    chmod +x "$INSTALL_DIR/clyean"
    finish_install
}

# ---------------------------------------------------------------------------
# Binary install (GitHub releases)
# ---------------------------------------------------------------------------

# Print the SHA-256 of a file with whichever tool the host provides, or nothing
# when neither sha256sum nor shasum exists.
file_sha256() {
    if has_command sha256sum; then
        sha256sum "$1" | awk '{print $1}'
    elif has_command shasum; then
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

verify_checksum() {
    file="$1"
    asset="$2"
    sums="$3"
    actual="$(file_sha256 "$file")"
    if [ -z "$actual" ]; then
        echo "Neither sha256sum nor shasum is available; skipping checksum verification."
        return 0
    fi
    expected="$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1; exit }' "$sums")"
    if [ -z "$expected" ]; then
        echo "SHA256SUMS for this release has no entry for ${asset}."
        exit 1
    fi
    if [ "$actual" != "$expected" ]; then
        echo "Checksum mismatch for ${asset}:"
        echo "    expected ${expected}"
        echo "    actual   ${actual}"
        exit 1
    fi
    echo "Checksum verified."
}

install_binary() {
    PLATFORM="$(host_platform)"
    ARCH="$(host_arch)"
    case "$ARCH" in
        x64|arm64) ;;
        *)         echo "Unsupported architecture: $ARCH"; exit 1 ;;
    esac

    ASSET="clyean-${PLATFORM}-${ARCH}"
    TAG="$(release_tag)"
    echo "Using version: $TAG"

    TMP_DIR="$(mktemp -d)"
    trap 'rm -rf "$TMP_DIR"' EXIT

    echo "Downloading ${ASSET}..."
    curl -fsSL --connect-timeout 10 --speed-limit 1024 --speed-time 30 "${GITHUB_DOWNLOAD}/${TAG}/${ASSET}" -o "${TMP_DIR}/${ASSET}"
    curl -fsSL --connect-timeout 10 --max-time 60 "${GITHUB_DOWNLOAD}/${TAG}/SHA256SUMS" -o "${TMP_DIR}/SHA256SUMS"
    verify_checksum "${TMP_DIR}/${ASSET}" "$ASSET" "${TMP_DIR}/SHA256SUMS"

    mkdir -p "$INSTALL_DIR"
    mv "${TMP_DIR}/${ASSET}" "${INSTALL_DIR}/clyean"
    chmod +x "${INSTALL_DIR}/clyean"
    finish_install
}

# Verify the freshly installed binary can actually start before reporting
# success. Never claim success for a binary that cannot run.
finish_install() {
    if ! SMOKE_OUTPUT="$("${INSTALL_DIR}/clyean" --version 2>&1)"; then
        echo ""
        echo "✗ clyean was installed to ${INSTALL_DIR}/clyean but cannot start:"
        echo "$SMOKE_OUTPUT" | sed 's/^/    /'
        if [ "$MODE" = "binary" ] && [ -f /etc/alpine-release ]; then
            echo ""
            echo "The prebuilt Linux binary targets glibc. On musl systems such as Alpine, build from source:"
            echo "    curl -fsSL https://clyean.com/install.sh | sh -s -- --source"
        fi
        exit 1
    fi

    echo ""
    echo "✓ Installed ${SMOKE_OUTPUT} to ${INSTALL_DIR}/clyean"

    case ":$PATH:" in
        *":$INSTALL_DIR:"*) echo "Run 'clyean' inside a project directory to get started!" ;;
        *) echo "Add ${INSTALL_DIR} to your PATH, then run 'clyean' inside a project directory" ;;
    esac
}

# Main logic
ensure_dependencies

case "$MODE" in
    source) install_via_cargo ;;
    binary) install_binary ;;
esac
