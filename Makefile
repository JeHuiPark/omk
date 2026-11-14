.PHONY: build test clean

build:
	cargo build --release

test:
	cargo test --workspace

clean:
	cargo clean
