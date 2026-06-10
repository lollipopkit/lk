NPM ?= npm
INSTALL_VSIX ?=
VSCODE_CLI ?=

VSC_EXT_DIR := vsc-ext
VSC_EXTENSIONS := lsp

.PHONY: vsix $(VSC_EXTENSIONS:%=vsix-%) clean-vsix debug-lsp-ext install-lk

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

install-lk:
	cargo install --path cli
	cargo install --path lsp
