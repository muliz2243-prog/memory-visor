# Agent Verification Instructions

To verify the project state, run the following commands sequentially:

1. Format check:
   ```bash
   cargo fmt --check
   ```

2. Comprehensive test suite:
   ```bash
   cargo test --all-targets
   ```

3. Linter verification:
   ```bash
   cargo clippy --all-targets -- -D warnings
   ```
