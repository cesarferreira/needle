.PHONY: test check fmt lint run demo clean

test:
	cargo nextest run

check:
	cargo check

fmt:
	cargo fmt --all

lint:
	cargo clippy --all-targets --all-features -D warnings

run:
	cargo run --

demo:
	cargo run -- --demo

clean:
	cargo clean
