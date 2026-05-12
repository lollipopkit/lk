NPM ?= npm
NPM_CACHE ?= /private/tmp/lkr-npm-cache

VSC_EXT_DIR := vsc-ext
VSC_EXTENSIONS := highlight lsp

.PHONY: vsix $(VSC_EXTENSIONS:%=vsix-%) clean-vsix debug-lsp-ext

vsix: $(VSC_EXTENSIONS:%=vsix-%)

$(VSC_EXTENSIONS:%=vsix-%): vsix-%:
	$(NPM) ci --cache $(NPM_CACHE) --prefix $(VSC_EXT_DIR)/$*
	$(NPM) --cache $(NPM_CACHE) --prefix $(VSC_EXT_DIR)/$* run package

clean-vsix:
	rm -f $(VSC_EXT_DIR)/*/*.vsix

debug-lsp-ext:
	./scripts/debug-vscode-lsp.sh
