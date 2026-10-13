# harn task runner (llmux conventions)

# Format + lint + test — the gate before every commit
check:
    cargo fmt --check
    cargo clippy --all-targets --locked -- -D warnings
    cargo test --locked

# Run tests
test:
    cargo test --locked

# Build release binary
build:
    cargo build --release --locked

# Format the tree
fmt:
    cargo fmt
