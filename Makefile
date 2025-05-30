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

client:
	cd js && npm run dev

client-prod:
	cd js && npm run prod

chrome:
	./scripts/start-localhost-test-chrome.sh


test:
	cargo test

