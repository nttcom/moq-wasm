RUSTFLAGS := --cfg tokio_unstable --remap-path-prefix=$(shell pwd)=.

# デフォルトターゲット（何も指定しない場合）
default: run

# 実行用
server:
	RUSTFLAGS="$(RUSTFLAGS)" cargo run -p moqt-server-sample

server-trace:
	RUSTFLAGS="$(RUSTFLAGS)" cargo run -p moqt-server-sample -- --log trace

server-warn:
	RUSTFLAGS="$(RUSTFLAGS)" cargo run -p moqt-server-sample -- --log warn

server-release:
	RUSTFLAGS="$(RUSTFLAGS)" cargo run -p moqt-server-sample --release

server-flamegraph:
	RUSTFLAGS="$(RUSTFLAGS)" cargo flamegraph -p moqt-server-sample --release

client:
	cd js && npm run dev

client-prod:
	cd js && npm run prod

ingest-gateway:
	RUSTFLAGS="$(RUSTFLAGS)" cargo run -p moqt-ingest-gateway -- \
		--rtmp-addr 0.0.0.0:1935 \
		--srt-addr 0.0.0.0:9000 \
		--moqt-url https://moqt.research.skyway.io:4433

ffmpeg-rtmp:
	ffmpeg -re \
		-f lavfi -i "testsrc=size=1920x1080:rate=30" \
		-f lavfi -i "sine=frequency=1000:sample_rate=48000" \
		-c:v libx264 -preset veryfast -profile:v baseline -pix_fmt yuv420p \
		-g 60 -sc_threshold 0 \
		-c:a aac -ar 48000 -ac 2 \
		-f flv "rtmp://localhost:1935/live/test"

ffmpeg-rtmp-local-pub:
	ffmpeg -re \
		-f lavfi -i "testsrc=size=1920x1080:rate=30" \
		-f lavfi -i "sine=frequency=1000:sample_rate=48000" \
		-c:v libx264 -preset veryfast -profile:v baseline -pix_fmt yuv420p \
		-g 60 -sc_threshold 0 \
		-c:a aac -ar 48000 -ac 2 \
		-map 0:v:0 -map 1:a:0 \
		-f tee "[f=flv:onfail=ignore]rtmp://localhost:1935/live/main|[f=flv:onfail=ignore]rtmp://localhost:1936/live/monitor"

ffmpeg-rtmp-local-sub:
	ffmpeg -listen 1 -i rtmp://0.0.0.0:1936/live/monitor -c copy -f matroska - \
	| ffplay -fflags +nobuffer -flags low_delay -

chrome:
	./scripts/chrome_mac.sh


test:
	cargo test

