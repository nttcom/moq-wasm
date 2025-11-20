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

chrome:
	./scripts/chrome_mac.sh


test:
	cargo test

