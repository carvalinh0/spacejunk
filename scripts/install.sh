#!/bin/sh
# Installs spacejunk on Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/carvalinh0/spacejunk/main/scripts/install.sh | sh
#
# 1. With cargo available: `cargo install spacejunk` (built for this machine).
# 2. Without cargo, or if that build fails: downloads the latest GitHub release
#    binary, checks its SHA-256 and installs it to ~/.local/bin.
# 3. Adds an app menu entry that opens spacejunk in a terminal:
#    ~/.local/share/applications/spacejunk.desktop (Linux) or
#    ~/Applications/spacejunk.app (macOS).
#
# Running it again updates to the latest version.

set -eu

REPO="carvalinh0/spacejunk"
BIN_DIR="$HOME/.local/bin"
ASSETS_URL="https://raw.githubusercontent.com/$REPO/main/assets"

say() { printf 'spacejunk: %s\n' "$*"; }
fail() { say "error: $*" >&2; exit 1; }

install_with_cargo() {
    command -v cargo >/dev/null 2>&1 || return 1
    say "installing with cargo (cargo install spacejunk)..."
    cargo install spacejunk --locked
}

detect_target() {
    case "$(uname -s)" in
        Linux) os="unknown-linux-musl" ;;
        Darwin) os="apple-darwin" ;;
        *) fail "unsupported system $(uname -s); on Windows use install.ps1" ;;
    esac
    case "$(uname -m)" in
        x86_64 | amd64) arch="x86_64" ;;
        aarch64 | arm64) arch="aarch64" ;;
        *) fail "unsupported CPU $(uname -m)" ;;
    esac
    echo "$arch-$os"
}

# download URL FILE
download() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$1" -O "$2"
    else
        fail "neither curl nor wget is installed"
    fi
}

sha256_check() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c "$1" >/dev/null
    else
        shasum -a 256 -c "$1" >/dev/null
    fi
}

install_from_release() {
    target="$(detect_target)"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT

    download "https://api.github.com/repos/$REPO/releases/latest" "$tmp/latest.json" ||
        fail "could not reach the GitHub releases of $REPO"
    tag="$(sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' "$tmp/latest.json" | head -n 1)"
    [ -n "$tag" ] || fail "no release found for $REPO"

    asset="spacejunk-$tag-$target.tar.gz"
    say "downloading $asset..."
    url="https://github.com/$REPO/releases/download/$tag/$asset"
    download "$url" "$tmp/$asset" || fail "no binary for $target in release $tag"
    download "$url.sha256" "$tmp/$asset.sha256" || fail "checksum missing for $asset"
    (cd "$tmp" && sha256_check "$asset.sha256") || fail "checksum mismatch for $asset"

    tar -xzf "$tmp/$asset" -C "$tmp"
    mkdir -p "$BIN_DIR"
    # Copy-then-rename so a running spacejunk is never left half-written.
    cp "$tmp/spacejunk" "$BIN_DIR/.spacejunk.new"
    chmod 755 "$BIN_DIR/.spacejunk.new"
    mv -f "$BIN_DIR/.spacejunk.new" "$BIN_DIR/spacejunk"
    say "installed $tag to $BIN_DIR/spacejunk"

    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) say "add $BIN_DIR to your PATH, e.g.: echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.profile" ;;
    esac
}

# download_icon NAME DEST — best effort: a launcher without an icon still works.
# The subshell keeps `fail` (no curl/wget) from aborting an install that succeeded.
download_icon() {
    (download "$ASSETS_URL/$1" "$2") 2>/dev/null || rm -f "$2"
}

linux_launcher() {
    data="${XDG_DATA_HOME:-$HOME/.local/share}"
    icons="$data/icons/hicolor/scalable/apps"
    mkdir -p "$data/applications" "$icons"
    download_icon logo.svg "$icons/spacejunk.svg"
    cat >"$data/applications/spacejunk.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=spacejunk
GenericName=Disk Usage Analyzer
Comment=Find out what is eating your disk
Exec="$1"
Icon=spacejunk
Terminal=true
Categories=System;Utility;FileTools;
Keywords=disk;space;usage;du;storage;
EOF
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$data/applications" >/dev/null 2>&1 || true
    fi
    say "added spacejunk to the app menu"
}

# A minimal bundle whose only job is to open Terminal running the binary.
macos_launcher() {
    app="$HOME/Applications/spacejunk.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    download_icon spacejunk.icns "$app/Contents/Resources/spacejunk.icns"
    cat >"$app/Contents/MacOS/spacejunk-launcher" <<EOF
#!/bin/sh
exec open -a Terminal "$1"
EOF
    chmod 755 "$app/Contents/MacOS/spacejunk-launcher"
    cat >"$app/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>spacejunk</string>
    <key>CFBundleDisplayName</key><string>spacejunk</string>
    <key>CFBundleIdentifier</key><string>io.github.carvalinh0.spacejunk</string>
    <key>CFBundleExecutable</key><string>spacejunk-launcher</string>
    <key>CFBundleIconFile</key><string>spacejunk</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>1.0</string>
    <key>LSMinimumSystemVersion</key><string>10.13</string>
</dict>
</plist>
EOF
    touch "$app" # makes Finder pick up the new icon
    say "added spacejunk to ~/Applications (Launchpad and Spotlight)"
}

# create_launcher BINARY — never fails the install: spacejunk itself is in place.
create_launcher() {
    case "$(uname -s)" in
        Linux) linux_launcher "$1" ;;
        Darwin) macos_launcher "$1" ;;
    esac || say "could not create the app menu entry; run spacejunk from a terminal"
}

if install_with_cargo; then
    create_launcher "${CARGO_HOME:-$HOME/.cargo}/bin/spacejunk"
else
    command -v cargo >/dev/null 2>&1 && say "cargo install failed; falling back to the prebuilt binary"
    install_from_release
    create_launcher "$BIN_DIR/spacejunk"
fi
say "done. Run: spacejunk (or open it from your app menu)"
