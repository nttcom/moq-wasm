export RUSTFLAGS := --cfg tokio_unstable --cfg web_sys_unstable_apis --remap-path-prefix=$(shell pwd)=.

-include .env

LOCAL_MOQT_URL ?= $(shell node scripts/resolve-local-relay-url.mjs "$(MOQT_URL)")
LIVE_INGEST_MOQT_URL ?= $(LOCAL_MOQT_URL)
ONVIF_MOQT_URL ?= $(LOCAL_MOQT_URL)

.PHONY: relay browser chrome chrome\:linux live-ingest onvif onvif-controller ffmpeg-rtmp test lint format relay-certs browser-e2e-media browser-e2e-call browser-e2e-call-headed

# Applications
relay:
	set -a; [ ! -f .env ] || . ./.env; set +a; RUSTFLAGS="$(RUSTFLAGS)" cargo run -p relay

browser:
	cd examples/browser && npm run dev

chrome:
	./scripts/chrome_mac.sh

chrome\:linux:
	./scripts/chrome_linux.sh

# RTMP/SRT Bridges
live-ingest:
	@echo "Using MoQT relay URL: $(LIVE_INGEST_MOQT_URL)"
	RUSTFLAGS="$(RUSTFLAGS)" cargo run -p moqt-bridge-live-ingest -- \
		--rtmp-addr 0.0.0.0:1935 \
		--srt-addr 0.0.0.0:9000 \
		--moqt-url $(LIVE_INGEST_MOQT_URL)

## Media helpers
relay-certs:
	node scripts/ensure-relay-certs.mjs

ffmpeg-rtmp:
	ffmpeg -re \
		-f lavfi -i "testsrc=size=1920x1080:rate=30" \
		-f lavfi -i "sine=frequency=1000:sample_rate=48000" \
		-c:v libx264 -preset veryfast -profile:v baseline -pix_fmt yuv420p \
		-g 60 -sc_threshold 0 \
		-c:a aac -ar 48000 -ac 2 \
		-f flv "rtmp://localhost:1935/live/test"

# ONVIF Bridges
onvif:
	@if [ -z "$(ONVIF_IP)" ] || [ -z "$(ONVIF_USERNAME)" ] || [ -z "$(ONVIF_PASSWORD)" ]; then \
		echo "ONVIF_IP/ONVIF_USERNAME/ONVIF_PASSWORD are required (set in .env or environment)"; \
		exit 1; \
	fi
	@echo "Using MoQT relay URL: $(ONVIF_MOQT_URL)"
	RUSTFLAGS="$(RUSTFLAGS)" cargo run -p moqt-bridge-onvif --bin moqt-onvif-client -- \
		--ip $(ONVIF_IP) \
		--username $(ONVIF_USERNAME) \
		--password $(ONVIF_PASSWORD) \
		--moqt-url $(ONVIF_MOQT_URL) \
		--publish-namespace onvif/client \
		--subscribe-namespace onvif/viewer \
		--video-track video \
		--audio-track audio \
		--catalog-track catalog \
		--command-track command \
		--dump-keyframe \
		--payload-format avcc \
		--insecure-skip-tls-verify

onvif-controller:
	@if [ -z "$(ONVIF_IP)" ] || [ -z "$(ONVIF_USERNAME)" ] || [ -z "$(ONVIF_PASSWORD)" ]; then \
		echo "ONVIF_IP/ONVIF_USERNAME/ONVIF_PASSWORD are required (set in .env or environment)"; \
		exit 1; \
	fi
	RUSTFLAGS="$(RUSTFLAGS)" cargo run -p moqt-bridge-onvif --bin moqt-bridge-onvif -- \
		--ip $(ONVIF_IP) \
		--username $(ONVIF_USERNAME) \
		--password $(ONVIF_PASSWORD)


# Maintenance
test:
	cargo test

lint:
	cargo clippy --workspace --all-targets --all-features --fix
	cd examples/browser && npm run lint

format:
	cargo fmt --all
	npx --prefix examples/browser prettier --write "examples/browser/**/*.{js,jsx,ts,tsx,json,css,md}" --ignore-path examples/browser/.prettierignore

browser-e2e-media:
	node scripts/setup-media-e2e.mjs
	node scripts/run-media-e2e.mjs

# Two-relay call E2E: brings up relay-a/relay-b via docker compose, then Playwright.
browser-e2e-call:
	node scripts/setup-media-e2e.mjs
	node scripts/run-call-e2e.mjs

# Same as browser-e2e-call but with a visible browser to watch behavior.
# Assumes setup already ran once (via browser-e2e-call or setup-media-e2e.mjs).
browser-e2e-call-headed:
	PLAYWRIGHT_HEADLESS=false node scripts/run-call-e2e.mjs
