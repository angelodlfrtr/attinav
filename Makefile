# Entry point for the whole repository. The firmware and the client each have their own Nix dev
# shell and Makefile under software/; this one delegates so `make check` at the root runs both.
# Use `make -C software/firmware help` / `make -C software/client help` for the full lists.

FIRMWARE := software/firmware
CLIENT   := software/client

.DEFAULT_GOAL := help

.PHONY: help
help: ## list the targets
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z_-]+:.*## / {printf "  %-16s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

# --- firmware ---------------------------------------------------------------------------------

.PHONY: fw-build
fw-build: ## build the application firmware
	$(MAKE) -C $(FIRMWARE) build

.PHONY: fw-boot
fw-boot: ## build the bootloader
	$(MAKE) -C $(FIRMWARE) boot

.PHONY: fw-bin
fw-bin: ## raw OTA image -> software/firmware/target/attinav-fw.bin
	$(MAKE) -C $(FIRMWARE) bin

.PHONY: fw-flash
fw-flash: ## flash the application over SWD with defmt logs
	$(MAKE) -C $(FIRMWARE) flash

.PHONY: fw-flash-boot
fw-flash-boot: ## flash the bootloader over SWD
	$(MAKE) -C $(FIRMWARE) flash-boot

.PHONY: fw-test
fw-test: ## firmware host-side tests
	$(MAKE) -C $(FIRMWARE) test

.PHONY: fw-lint
fw-lint: ## firmware clippy
	$(MAKE) -C $(FIRMWARE) lint

.PHONY: fw-check
fw-check: ## firmware: fmt-check, lint, test, build
	$(MAKE) -C $(FIRMWARE) check

# --- client -----------------------------------------------------------------------------------

.PHONY: client-dev
client-dev: ## run the desktop client with hot reload
	$(MAKE) -C $(CLIENT) dev

.PHONY: client-build
client-build: ## client release binary
	$(MAKE) -C $(CLIENT) build

.PHONY: client-bundle
client-bundle: ## client release bundles
	$(MAKE) -C $(CLIENT) bundle

.PHONY: client-test
client-test: ## client Rust tests (incl. end-to-end against the simulator)
	$(MAKE) -C $(CLIENT) test

.PHONY: client-lint
client-lint: ## client tsc + eslint + clippy
	$(MAKE) -C $(CLIENT) lint

.PHONY: client-check
client-check: ## client: fmt-check, lint, test, build
	$(MAKE) -C $(CLIENT) check

# --- everything -------------------------------------------------------------------------------

.PHONY: fmt
fmt: ## format both projects
	$(MAKE) -C $(FIRMWARE) fmt
	$(MAKE) -C $(CLIENT) fmt

.PHONY: test
test: fw-test client-test ## all tests

.PHONY: lint
lint: fw-lint client-lint ## all linters

.PHONY: check
check: fw-check client-check ## everything CI would run, both projects

.PHONY: clean
clean: ## drop build outputs of both projects
	$(MAKE) -C $(FIRMWARE) clean
	$(MAKE) -C $(CLIENT) clean
