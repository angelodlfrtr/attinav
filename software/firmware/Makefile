# Common operations for the firmware. Every target runs with the tools the Nix dev shell
# provides; from outside the shell the recipe enters it first (`path:` because the tree may be
# untracked in git, which a plain `nix develop` would refuse to see).

RUN := $(if $(IN_NIX_SHELL),,nix develop path:$(CURDIR) -c)

# probe-rs chip name, overridable for a different package of the same family.
CHIP ?= STM32U575RG

.DEFAULT_GOAL := help

.PHONY: help
help: ## list the targets
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z_-]+:.*## / {printf "  %-12s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

.PHONY: build
build: ## build the application firmware (release, thumbv8m.main-none-eabihf)
	$(RUN) cargo build --release

.PHONY: boot
boot: ## build the bootloader
	$(RUN) cargo build --release -p attinav-boot

.PHONY: bin
bin: ## raw application image for an OTA update -> target/attinav-fw.bin
	$(RUN) cargo fw-bin

.PHONY: size
size: ## section sizes of the application and the bootloader
	$(RUN) cargo fw-size
	$(RUN) cargo size --release -p attinav-boot -- -A

.PHONY: flash
flash: ## flash the application over SWD and stream defmt logs (probe-rs)
	$(RUN) cargo run --release

.PHONY: flash-boot
flash-boot: ## flash the bootloader over SWD (once per board, or when memory.x changes)
	$(RUN) cargo boot-flash

.PHONY: erase
erase: ## erase the whole flash (bootloader, application, state, calibration)
	$(RUN) probe-rs erase --chip $(CHIP)

.PHONY: test
test: ## host-side unit tests of attinav-core and attinav-sensors
	$(RUN) cargo test-host

.PHONY: lint
lint: ## clippy (pedantic, -D warnings) on the firmware target and on the host libraries
	$(RUN) cargo lint
	$(RUN) cargo lint-host

.PHONY: fmt
fmt: ## format the Rust code and the flake
	$(RUN) cargo fmt --all
	$(RUN) nixpkgs-fmt flake.nix

.PHONY: fmt-check
fmt-check: ## fail if anything is not formatted
	$(RUN) cargo fmt --all --check
	$(RUN) nixpkgs-fmt --check flake.nix

.PHONY: doc
doc: ## rustdoc of the host libraries, opened in the browser
	$(RUN) cargo doc --target x86_64-unknown-linux-gnu -p attinav-core -p attinav-sensors --features attinav-core/std --no-deps --open

.PHONY: audit
audit: ## cargo-audit over the dependency tree
	$(RUN) cargo audit

.PHONY: check
check: fmt-check lint test build boot ## everything CI would run

.PHONY: clean
clean: ## drop the build directory
	$(RUN) cargo clean
