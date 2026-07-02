NPM ?= npm
INSTALL_VSIX ?=
VSCODE_CLI ?=

VSC_EXT_DIR := ecosystem/vsc-ext
VSC_EXTENSIONS := lsp
ZED_EXT_DIR := ecosystem/zed-ext

.PHONY: vsix $(VSC_EXTENSIONS:%=vsix-%) clean-vsix debug-lsp-ext zed-ext-check install

vsix: $(VSC_EXTENSIONS:%=vsix-%)

$(VSC_EXTENSIONS:%=vsix-%): vsix-%:
	$(NPM) install --prefix $(VSC_EXT_DIR)/$*
	$(NPM) --prefix $(VSC_EXT_DIR)/$* run package
	@vsix_file=$$(ls -t $(VSC_EXT_DIR)/$*/*.vsix 2>/dev/null | head -n 1); \
	if [ -z "$$vsix_file" ]; then \
		echo "No VSIX package found under $(VSC_EXT_DIR)/$*"; \
		exit 1; \
	fi; \
	if [ "$(INSTALL_VSIX)" = "1" ] || [ "$(INSTALL_VSIX)" = "yes" ]; then \
		answer=yes; \
	elif [ -t 0 ]; then \
		printf "Install $$vsix_file into VS Code now? [y/N] "; \
		read answer || answer=; \
	else \
		echo "VSIX built: $$vsix_file"; \
		echo "Install it with: code --install-extension $$vsix_file"; \
		answer=no; \
	fi; \
	case "$$answer" in \
		[Yy]|[Yy][Ee][Ss]) \
			vscode_cli="$(VSCODE_CLI)"; \
			if [ -z "$$vscode_cli" ] && command -v code >/dev/null 2>&1; then \
				vscode_cli=code; \
			fi; \
			if [ -z "$$vscode_cli" ] && [ -x "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code" ]; then \
				vscode_cli="/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"; \
			fi; \
			if [ -z "$$vscode_cli" ] && [ -x "/Applications/Visual Studio Code - Insiders.app/Contents/Resources/app/bin/code-insiders" ]; then \
				vscode_cli="/Applications/Visual Studio Code - Insiders.app/Contents/Resources/app/bin/code-insiders"; \
			fi; \
			if [ -z "$$vscode_cli" ] && [ -x "/Applications/VSCodium.app/Contents/Resources/app/bin/codium" ]; then \
				vscode_cli="/Applications/VSCodium.app/Contents/Resources/app/bin/codium"; \
			fi; \
			if [ -n "$$vscode_cli" ]; then \
				"$$vscode_cli" --install-extension "$$vsix_file" || { \
					echo "VSIX install failed. The package was still built at: $$vsix_file"; \
					echo "If VS Code asks for a restart before reinstalling, restart VS Code and run: \"$$vscode_cli\" --install-extension $$vsix_file"; \
					exit 1; \
				}; \
			else \
				echo "VS Code CLI was not found. Install manually from VS Code: Extensions > ... > Install from VSIX... > $$vsix_file"; \
				echo "Or run: make vsix INSTALL_VSIX=1 VSCODE_CLI=/path/to/code"; \
				exit 1; \
			fi; \
			;; \
		*) \
			echo "Skipped VSIX install: $$vsix_file"; \
			;; \
	esac

clean-vsix:
	rm -f $(VSC_EXT_DIR)/*/*.vsix

debug-lsp-ext:
	./scripts/debug-vscode-lsp.sh

zed-ext-check:
	cargo check --manifest-path $(ZED_EXT_DIR)/Cargo.toml --target wasm32-wasip1

install:
	cargo install --path cli --force
	cargo install --path lsp --force
	$(MAKE) vsix INSTALL_VSIX=1

# Correctness harnesses (see plan.md). Miri needs `rustup component add miri
# --toolchain nightly`. Leaks are ignored because lkrt's arena ownership frees
# strings/containers via lkrt_cleanup() at process exit, which unit tests
# sharing the global arena must not call; Stacked Borrows UB checking stays on.
miri-lkrt:
	MIRIFLAGS="-Zmiri-disable-isolation -Zmiri-ignore-leaks" cargo +nightly miri test -p lkrt

# Differential corpora with the native side compiled under ASan/UBSan.
sanitized-differential:
	LK_NATIVE_SANITIZE=address,undefined cargo test -p lk-cli --test aot_differential_test
	LK_NATIVE_SANITIZE=address,undefined cargo test -p lk-cli --test examples_differential_test
	LK_NATIVE_SANITIZE=address,undefined LK_FUZZ_CASES=120 cargo test -p lk-cli --test aot_fuzz_differential_test

# Whole-suite GC stress: force a collection at every VM safepoint.
gc-stress:
	LK_GC_STRESS=1 cargo test -p lk-core -p lk-stdlib -p lk-cli
