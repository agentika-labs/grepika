# Contributing

## Development

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Run all benchmarks
cargo bench

# Run real-repo benchmarks (indexes and searches this repo)
cargo bench --bench hot_paths -- real_repo

# Run real-repo benchmarks against a different repository
BENCH_REPO_PATH=/path/to/repo cargo bench --bench hot_paths -- real_repo
```

## Profiling

Pass `--log-file` to enable timing and memory logging for each tool invocation:

```json
{
  "mcpServers": {
    "grepika": {
      "command": "grepika",
      "args": ["--mcp", "--log-file", "/tmp/grepika.log"]
    }
  }
}
```

Then: `tail -f /tmp/grepika.log`

```
[search] 42ms | mem: 128.5MB (+2.1MB) | ~91 tokens (0.4KB)
[index] 1.2s | mem: 256.0MB (+127.5MB) | ~150 tokens (0.6KB)
```

When `--log-file` is not provided, profiling is disabled with negligible overhead (~20ns per tool call).
