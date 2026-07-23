#!/usr/bin/env bash
# Generate Dart protobuf types from proto/ into lib/src/gen/.
# (Rust side is generated automatically by rust/build.rs via prost.)
set -euo pipefail
cd "$(dirname "$0")/.."

export PATH="$HOME/.pub-cache/bin:$PATH"
command -v protoc >/dev/null || { echo "protoc not found — run scripts/setup.sh"; exit 1; }
command -v protoc-gen-dart >/dev/null || { echo "protoc-gen-dart not found — run: dart pub global activate protoc_plugin"; exit 1; }

mkdir -p lib/src/gen
protoc --dart_out=lib/src/gen -Iproto proto/*.proto
echo "Generated Dart protobuf types into lib/src/gen/"
