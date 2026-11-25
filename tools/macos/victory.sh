#!/bin/sh
# Builds VIctory for Windows and runs it on macOS under CrossOver Wine with
# Apple's D3DMetal. Arguments go to victory.exe, for example:
#   tools/macos/victory.sh config
#   tools/macos/victory.sh run --execute <path to the game's executable>
# WINE defaults to the Wine that tools/macos/build-crossover-wine.sh builds.
set -e
cd "$(dirname "$0")/../.."
ROOT="$PWD"
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
WINE="${WINE:-$HOME/Library/Caches/victory-macos/target/wine-cx/bin/wine}"
[ -x "$WINE" ] || { echo "no Wine at $WINE: run tools/macos/build-crossover-wine.sh first" >&2; exit 1; }
[ -d "$(dirname "$WINE")/../lib/external/D3DMetal.framework" ] || {
    echo "D3DMetal is missing: copy the redist/lib folder of Apple's evaluation environment over $(dirname "$WINE")/../lib" >&2
    exit 1
}
cargo build --quiet --release -p victory --target x86_64-pc-windows-gnullvm
export WINEPREFIX="${WINEPREFIX:-$ROOT/target/wineprefix-play}"
export WINEDEBUG="${WINEDEBUG:--all}"
export WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d;d3d12,d3d12core,dxgi=b${WINEDLLOVERRIDES:+;$WINEDLLOVERRIDES}"
# Rosetta tells the game about AVX and AVX2, which it may check for.
export ROSETTA_ADVERTISE_AVX=1
# What runs best on a Mac. Title memory is the card's own (VICTORY_INPLACE,
# with nocopy.m's library in Wine; 0, or without that file: copies of it), the big buffers with it
# (a copy looked at once a frame showed old bytes), and textures take the
# memory of the levels they hold. The card's share is of the Mac's memory.
[ -f tools/macos/nocopy.m ] || export VICTORY_INPLACE=0
export VICTORY_INPLACE="${VICTORY_INPLACE:-3}" VICTORY_WINDOW_KB="${VICTORY_WINDOW_KB:-64}"
export VICTORY_LEVELS_ON_DEMAND="${VICTORY_LEVELS_ON_DEMAND:-1}"
export VICTORY_CARD_MB="${VICTORY_CARD_MB:-$(($(sysctl -n hw.memsize) / 1048576 * 45 / 100))}"
export VICTORY_CARD_ROOM_FOR_TARGETS_MB="${VICTORY_CARD_ROOM_FOR_TARGETS_MB:-1400}"
if [ "$VICTORY_INPLACE" != 0 ]; then
    export VICTORY_CARD_BUFFERS="${VICTORY_CARD_BUFFERS:-0}" VICTORY_INDIRECT_BLOCKS="${VICTORY_INDIRECT_BLOCKS:-1}"
    export VICTORY_LATE_WAIT="${VICTORY_LATE_WAIT:-0}"
fi
# nocopy.m's library makes the card's buffers on title memory.
if [ "$VICTORY_INPLACE" != 0 ]; then
    LIB="$ROOT/target/macos/nocopy.dylib"
    mkdir -p "$ROOT/target/macos"
    [ "$LIB" -nt tools/macos/nocopy.m ] || clang -fno-objc-arc -arch x86_64 -dynamiclib -O2 -framework Metal -framework Foundation \
        tools/macos/nocopy.m -o "$LIB"
    export DYLD_INSERT_LIBRARIES="$LIB"
fi
exec "$WINE" "$ROOT/target/x86_64-pc-windows-gnullvm/release/victory.exe" "$@"
