#!/bin/bash
cargo build -p epoca --release && ./target/release/epoca "$@"
