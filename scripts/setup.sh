#!/usr/bin/env bash
# Hoppler dev environment setup (T03). Idempotent: checks before installing.
# Covers what can be done without root, and prints the one sudo command it needs.
set -euo pipefail

FRB_VERSION="2.12.0"
say() { printf '\033[1m== %s\033[0m\n' "$*"; }

say "System packages (need sudo — run yourself if this fails)"
MISSING_PKGS=()
command -v cmake >/dev/null || MISSING_PKGS+=(cmake)
command -v ninja >/dev/null || MISSING_PKGS+=(ninja)
command -v javac >/dev/null || MISSING_PKGS+=(jdk21-openjdk)
command -v protoc >/dev/null || MISSING_PKGS+=(protobuf)
if [ ${#MISSING_PKGS[@]} -gt 0 ]; then
  echo "Missing: ${MISSING_PKGS[*]}"
  echo "Run:  sudo pacman -S --needed ${MISSING_PKGS[*]}"
else
  echo "All present."
fi

say "Rust toolchain"
command -v rustup >/dev/null || { echo "Install rustup first: https://rustup.rs"; exit 1; }
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android

say "flutter_rust_bridge_codegen ${FRB_VERSION}"
if ! "$HOME/.cargo/bin/flutter_rust_bridge_codegen" --version 2>/dev/null | grep -q "$FRB_VERSION"; then
  cargo install flutter_rust_bridge_codegen --version "$FRB_VERSION" --locked
fi

say "protoc-gen-dart"
[ -x "$HOME/.pub-cache/bin/protoc-gen-dart" ] || dart pub global activate protoc_plugin

say "Android SDK"
if [ -d "${ANDROID_HOME:-$HOME/Android/Sdk}" ]; then
  echo "Found: ${ANDROID_HOME:-$HOME/Android/Sdk}"
else
  echo "Not installed. After the JDK is present, install cmdline-tools to ~/Android/Sdk,"
  echo "then: sdkmanager 'platform-tools' 'platforms;android-35' 'build-tools;35.0.0'"
  echo "and export ANDROID_HOME=\$HOME/Android/Sdk in your shell profile."
fi

say "Flutter deps"
flutter pub get

say "Done. Next: 'make gen' if you changed api/proto, 'make run' for Linux desktop."
