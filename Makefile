# VisionOps Core — developer commands.
CORE := apps/core/Cargo.toml

.PHONY: build release check fmt setup-mediamtx dev synth validate web-install web-dev clean

build:               ## Build the Rust control plane (debug)
	cargo build --manifest-path $(CORE)

release:             ## Build optimized binary
	cargo build --release --manifest-path $(CORE)

check:               ## Clippy lints
	cargo clippy --manifest-path $(CORE) --all-targets

fmt:                 ## Format Rust sources
	cargo fmt --manifest-path $(CORE)

setup-mediamtx:      ## Download the MediaMTX binary into infra/mediamtx/
	bash scripts/setup_mediamtx.sh

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
	cargo clean --manifest-path $(CORE)
