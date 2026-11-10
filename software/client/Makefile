# Common operations for the desktop client. Every target runs with the tools the Nix dev shell
# provides; from outside the shell the recipe enters it first.

RUN := $(if $(IN_NIX_SHELL),,nix develop path:$(CURDIR) -c)
CARGO := $(RUN) cargo

.DEFAULT_GOAL := help

.PHONY: help
help: ## list the targets
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z_-]+:.*## / {printf "  %-12s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

node_modules: package.json package-lock.json
	$(RUN) npm ci --no-audit --no-fund
	@touch node_modules

.PHONY: deps
deps: node_modules ## install the frontend dependencies (npm ci)

.PHONY: dev
dev: node_modules ## run the app with hot reload (Vite + Tauri)
	$(RUN) cargo tauri dev

.PHONY: build
build: node_modules ## release binary without installers -> src-tauri/target/release/attinav-client
	$(RUN) cargo tauri build --no-bundle

.PHONY: bundle
bundle: node_modules ## release bundles (deb, rpm, AppImage, ...) -> src-tauri/target/release/bundle
	$(RUN) cargo tauri build

.PHONY: frontend
frontend: node_modules ## type-check and build the frontend only -> dist/
	$(RUN) npm run build

.PHONY: typecheck
typecheck: node_modules ## tsc --noEmit
	$(RUN) npx tsc --noEmit -p tsconfig.json

.PHONY: eslint
eslint: node_modules ## eslint over src/
	$(RUN) npx eslint src

# `tauri::generate_context!` embeds dist/ at compile time, so every Rust build needs the frontend
# built first (a clean checkout has no dist/).
.PHONY: lint
lint: typecheck eslint frontend ## tsc + eslint on the frontend, clippy on the Rust side
	cd src-tauri && $(CARGO) lint

.PHONY: test
test: frontend ## Rust unit and end-to-end tests (link + simulator through the real protocol)
	cd src-tauri && $(CARGO) test

.PHONY: fmt
fmt: ## format the Rust code and the flake
	cd src-tauri && $(CARGO) fmt --all
	$(RUN) nixpkgs-fmt flake.nix

.PHONY: fmt-check
fmt-check: ## fail if anything is not formatted
	cd src-tauri && $(CARGO) fmt --all --check
	$(RUN) nixpkgs-fmt --check flake.nix

.PHONY: check
check: fmt-check lint test build ## everything CI would run

.PHONY: clean
clean: ## drop build outputs (keeps node_modules; `make distclean` drops those too)
	cd src-tauri && $(CARGO) clean
	rm -rf dist

.PHONY: distclean
distclean: clean ## clean plus node_modules
	rm -rf node_modules
