---
title: Contributing
---

### Local development

1. **Install Rust** (the latest stable version will work great)
2. Run `cargo build --release` to get better benchmarks.
3. Adjust the configuration in `pg_doorman.toml` to match your setup (this step is optional, given next).
4. Execute `cargo run --release`. You're now ready to go!
5. Also, you can use `make docker-compose-test-all` for testing with docker.