#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

target=x86_64-pc-windows-gnullvm
cache_dir="${CARGO_TARGET_DIR:-$HOME/afwshswc-build}"
mkdir -p "$cache_dir"
export CARGO_TARGET_DIR="$(cd "$cache_dir" && pwd)"

if [[ -z "${CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER:-}" ]]; then
    sdk=llvm-mingw-20261117-ucrt-ubuntu-22.04-x86_64
    linker="$CARGO_TARGET_DIR/$sdk/bin/x86_64-w64-mingw32-clang"
    if [[ ! -x "$linker" ]]; then
        archive="$CARGO_TARGET_DIR/$sdk.tar.xz"
        curl --fail --location --retry 1 \
            "https://github.com/mstorsjo/llvm-mingw/releases/download/20261117/$sdk.tar.xz" \
            --output "$archive"
        printf '%s  %s\n' e7a15a49a2ea95e9f3d8a9d0cfabe5052a4b10fb4777cc54de5a8c27b64bb701 "$archive" \
            | sha256sum --check
        tar -xf "$archive" -C "$CARGO_TARGET_DIR"
    fi
    export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER="$linker"
    export AFWSHSWC_RC="${AFWSHSWC_RC:-$CARGO_TARGET_DIR/$sdk/bin/llvm-rc}"
fi

rustup target add "$target"
cargo build --locked --release --target "$target"
mkdir -p dist
cp "$CARGO_TARGET_DIR/$target/release/AFWSHSWC.exe" dist/AFWSHSWC.exe
