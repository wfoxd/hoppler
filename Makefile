# Hoppler — task runner (T03). Thin wrappers; each target runnable in CI and locally.

FRB_VERSION := 2.12.0
export PATH := $(HOME)/.cargo/bin:$(HOME)/.pub-cache/bin:$(PATH)

.PHONY: setup gen lint lint-android test run apk ci

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

# The Android-only Rust — the JNI bridge to the hardware keystore — is
# compiled out on the host, so `lint` above has never seen it. It is not a
# theoretical gap: the first version of `platform_keystore` had a clippy error
# that only this target could find, and `make ci` was green over it.
#
# The NDK is where the C compiler for the target lives; sqlcipher is C and will
# not configure without it. `--target` only, not `--all-targets`: there are no
# Android-only tests to lint.
# ANDROID_HOME is exported by the GitHub runner and by a normal Studio install;
# the fallback is where the SDK lands when neither has set it. Highest NDK
# wins. Linux-only path — the one place this runs besides a dev box is CI.
NDK := $(lastword $(sort $(wildcard $(or $(ANDROID_HOME),$(HOME)/Android/Sdk)/ndk/*)))
NDK_BIN := $(NDK)/toolchains/llvm/prebuilt/linux-x86_64/bin

lint-android:     ## Lint the Rust that only exists on Android
	@test -n "$(NDK)" || { echo "no NDK under $(or $(ANDROID_HOME),$(HOME)/Android/Sdk)/ndk"; exit 1; }
	cd rust && CC_aarch64_linux_android=$(NDK_BIN)/aarch64-linux-android31-clang \
	  AR_aarch64_linux_android=$(NDK_BIN)/llvm-ar \
	  CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$(NDK_BIN)/aarch64-linux-android31-clang \
	  cargo clippy --target aarch64-linux-android -- -D warnings

test:             ## Rust + Dart tests
	cd rust && cargo test
	flutter test

run:              ## Run on Linux desktop (the everyday dev loop)
	flutter run -d linux

apk:              ## Android debug build
	flutter build apk --debug

ci: lint test     ## What CI runs — keep green before pushing
	./scripts/gen-proto.sh && git diff --exit-code lib/src/gen
