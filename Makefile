export RUSTFLAGS := --cfg tokio_unstable --cfg web_sys_unstable_apis --remap-path-prefix=$(shell pwd)=.

-include .env

.PHONY: relay browser chrome chrome\:linux live-ingest onvif onvif-controller ffmpeg-rtmp test lint format browser-e2e-media

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
	RUSTFLAGS="$(RUSTFLAGS)" cargo run -p moqt-bridge-live-ingest -- \
		--rtmp-addr 0.0.0.0:1935 \
		--srt-addr 0.0.0.0:9000 \
		--moqt-url https://127.0.0.1:4433

## Media helpers
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
	RUSTFLAGS="$(RUSTFLAGS)" cargo run -p moqt-bridge-onvif --bin moqt-onvif-client -- \
		--ip $(ONVIF_IP) \
		--username $(ONVIF_USERNAME) \
		--password $(ONVIF_PASSWORD) \
		--moqt-url $(MOQT_URL) \
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
	cd examples/browser && npm run e2e:media
