NPM ?= npm

VSC_EXT_DIR := vsc-ext
VSC_EXTENSIONS := highlight lsp

.PHONY: vsix $(VSC_EXTENSIONS:%=vsix-%) clean-vsix debug-lsp-ext install-lk

vsix: $(VSC_EXTENSIONS:%=vsix-%)

$(VSC_EXTENSIONS:%=vsix-%): vsix-%:
	$(NPM) install --prefix $(VSC_EXT_DIR)/$*
	$(NPM) --prefix $(VSC_EXT_DIR)/$* run package

clean-vsix:
	rm -f $(VSC_EXT_DIR)/*/*.vsix

debug-lsp-ext:
	./scripts/debug-vscode-lsp.sh

install-lk:
	cargo install --path cli
	cargo install --path lsp
