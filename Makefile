NPM ?= npm
CARGO ?= cargo
# Extra flags for the `cargo install` steps, e.g.
#   make install CARGO_INSTALL_FLAGS="--no-default-features --features stdlib"
# on a machine that cannot build the AOT backend.
CARGO_INSTALL_FLAGS ?=
# Set to a CLI path to install the VSIX into one specific editor instead of
# every VS Code-family editor found (see scripts/lib/vscode_cli.sh).
VSCODE_CLI ?=

VSC_EXT_DIR := ecosystem/vsc-ext
VSC_EXTENSIONS := lsp
ZED_EXT_DIR := ecosystem/zed-ext

.PHONY: vsix $(VSC_EXTENSIONS:%=vsix-%) clean-vsix debug-lsp-ext zed-ext-check zed-ext-release-check \
	install install-cli install-lsp install-vsix install-zed prune

vsix: $(VSC_EXTENSIONS:%=vsix-%)

# Packaging only. Installing is `make install-vsix`.
$(VSC_EXTENSIONS:%=vsix-%): vsix-%:
	$(NPM) install --prefix $(VSC_EXT_DIR)/$*
	$(NPM) --prefix $(VSC_EXT_DIR)/$* run package
	@vsix_file=$$(ls -t $(VSC_EXT_DIR)/$*/*.vsix 2>/dev/null | head -n 1); \
	if [ -z "$$vsix_file" ]; then \
		echo "No VSIX package found under $(VSC_EXT_DIR)/$*"; \
		exit 1; \
	fi; \
	echo "VSIX built: $$vsix_file"

clean-vsix:
	rm -f $(VSC_EXT_DIR)/*/*.vsix

# Reclaim `target/`. Cargo never removes the artifacts of a fingerprint it has
# stopped using, so the directory only grows: this workspace reached 190GB of
# `target/debug` before anyone looked. `cargo clean --gc` is still nightly-only.
prune:
	bash scripts/prune_target.sh

debug-lsp-ext:
	./scripts/debug-vscode-lsp.sh

zed-ext-check:
	cargo check --manifest-path $(ZED_EXT_DIR)/Cargo.toml --target wasm32-wasip1

# Run before publishing the Zed extension. `extension.toml` pins the grammar to
# a commit, and it ships with a placeholder — Zed clones that commit to build
# the grammar, so publishing with the placeholder in place produces an
# extension whose syntax highlighting cannot be built. A comment asking someone
# to remember is not a check; this is.
zed-ext-release-check: zed-ext-check
	@commit=$$(grep -E '^commit = ' $(ZED_EXT_DIR)/extension.toml | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	if ! printf '%s' "$$commit" | grep -qE '^[0-9a-f]{40}$$'; then \
		echo "zed extension.toml: grammar commit is '$$commit', not a 40-char SHA."; \
		echo "Set it to the commit that contains ecosystem/tree-sitter-lk before publishing."; \
		exit 1; \
	fi; \
	if ! git cat-file -e "$$commit^{commit}" 2>/dev/null; then \
		echo "zed extension.toml: grammar commit $$commit is not in this repository."; \
		exit 1; \
	fi; \
	echo "zed extension.toml: grammar pinned to $$commit"

# Everything a workstation needs: both binaries, the VS Code extension in every
# VS Code-family editor found (remote windows included), and the Zed step.
# The editor steps are best-effort by design — a machine without node or
# without an editor still gets a working `lk` and `lk-lsp`, and says so.
# The editor steps run under `||` so that a machine with no editor (or a failed
# VSIX install) still ends with `lk` and `lk-lsp` installed and a clear message,
# instead of aborting the target halfway. `make install-vsix` on its own keeps
# its non-zero exit for scripting.
install: install-cli install-lsp
	@$(MAKE) install-vsix || echo "install: the VS Code extension step failed (see above); lk and lk-lsp are installed."
	@$(MAKE) install-zed || true
	@echo "install: done. Reload your editor window to pick up the new extension and lk-lsp."

install-cli:
	$(CARGO) install --path cli --force $(CARGO_INSTALL_FLAGS)

install-lsp:
	$(CARGO) install --path lsp --force $(CARGO_INSTALL_FLAGS)

install-vsix:
	@if ! command -v $(NPM) >/dev/null 2>&1; then \
		echo "install-vsix: '$(NPM)' not found, skipping the VS Code extension."; \
		echo "install-vsix: install Node.js and rerun 'make install-vsix'."; \
		exit 0; \
	fi; \
	$(MAKE) vsix && VSCODE_CLI="$(VSCODE_CLI)" bash scripts/install_vsix.sh

install-zed:
	@bash scripts/install_zed_ext.sh

# Correctness harnesses (see plan.md). Miri needs `rustup component add miri
# --toolchain nightly`. Leaks are ignored because lkrt's arena ownership frees
# strings/containers via lkrt_cleanup() at process exit, which unit tests
# sharing the global arena must not call; Stacked Borrows UB checking stays on.
miri-lkrt:
	MIRIFLAGS="-Zmiri-disable-isolation -Zmiri-ignore-leaks" cargo +nightly miri test -p lkrt

# ASan-instrumented lkrt + the differential suites linked against it
# (scripts/build_lkrt_asan.sh; F1 harness, not a PR gate).
asan-lkrt:
	bash scripts/build_lkrt_asan.sh
	LKRT_STATICLIB=$(PWD)/target/lkrt-asan/x86_64-unknown-linux-gnu/release/liblkrt.a \
	LK_NATIVE_SANITIZE=address cargo test -p lk-cli --test aot_differential_test --test examples_differential_test

# Differential corpora with the native side compiled under ASan/UBSan.
sanitized-differential:
	LK_NATIVE_SANITIZE=address,undefined cargo test -p lk-cli --test aot_differential_test
	LK_NATIVE_SANITIZE=address,undefined cargo test -p lk-cli --test examples_differential_test
	LK_NATIVE_SANITIZE=address,undefined LK_FUZZ_CASES=120 cargo test -p lk-cli --test aot_fuzz_differential_test

# Whole-suite GC stress: force a collection at every VM safepoint.
gc-stress:
	LK_GC_STRESS=1 cargo test -p lk-core -p lk-stdlib -p lk-cli
