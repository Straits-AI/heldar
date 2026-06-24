# Heldar — developer commands (Cargo workspace).

.PHONY: build release check fmt test kernel setup-mediamtx dev synth validate web-install web-dev clean

build:               ## Build the workspace (debug)
	cargo build --workspace

release:             ## Build optimized binaries
	cargo build --release --workspace

check:               ## Clippy lints
	cargo clippy --workspace --all-targets

fmt:                 ## Format Rust sources
	cargo fmt --all

test:                ## Run the test suite
	cargo test --workspace

kernel:              ## Build the open kernel crate standalone (no proprietary apps)
	cargo build -p heldar-kernel

setup-mediamtx:      ## Download the MediaMTX binary into infra/mediamtx/
	bash scripts/setup_mediamtx.sh

appliance-image:     ## Build a native (no-Docker) appliance rootfs with systemd services
	bash scripts/build-appliance-image.sh

dev: build           ## Run MediaMTX + the control plane
	bash scripts/dev.sh

synth:               ## Publish a synthetic RTSP test camera to MediaMTX
	bash scripts/synth_camera.sh

validate: build      ## End-to-end kernel validation against a synthetic camera
	bash scripts/validate.sh

web-install:         ## Install dashboard dependencies
	cd apps/web && npm install

web-dev:             ## Run the dashboard dev server (proxies to :8000)
	cd apps/web && npm run dev

clean:               ## Remove Rust build output
	cargo clean
