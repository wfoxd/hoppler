# Hoppler — task runner (T03). Thin wrappers; each target runnable in CI and locally.

FRB_VERSION := 2.12.0
export PATH := $(HOME)/.cargo/bin:$(HOME)/.pub-cache/bin:$(PATH)

.PHONY: setup gen lint test run apk ci

setup:            ## One-command environment setup (idempotent)
	./scripts/setup.sh

gen:              ## Regenerate Dart<->Rust bridge and protobuf types
	flutter_rust_bridge_codegen generate
	./scripts/gen-proto.sh

# --all-targets, because bare `cargo clippy` lints the library and nothing
# else. Tests, benches and examples went unlinted from the first commit, and
# five findings had accumulated there unseen — including dead code and an
# assertion that could not fail. Test code is where the project's claims about
# itself live, so it is the last place worth leaving unchecked.
lint:             ## All linters, warnings are errors
	cd rust && cargo fmt --check && cargo clippy --all-targets -- -D warnings
	flutter analyze

test:             ## Rust + Dart tests
	cd rust && cargo test
	flutter test

run:              ## Run on Linux desktop (the everyday dev loop)
	flutter run -d linux

apk:              ## Android debug build
	flutter build apk --debug

ci: lint test     ## What CI runs — keep green before pushing
	./scripts/gen-proto.sh && git diff --exit-code lib/src/gen
