```
  _____ _______ _____            _____    _____ _   _
 / ____|__   __|  __ \     /\   |  __ \  |_   _| \ | |
| (___    | |  | |__) |   /  \  | |__) |   | | |  \| |
 \___ \   | |  |  _  /   / /\ \ |  ___/    | | | . ` |
 ____) |  | |  | | \ \  / ____ \| |       _| |_| |\  |
|_____/   |_|  |_|  \_\/_/    \_\_|      |_____|_| \_|
```

A fast, minimal coding agent harness in Rust. Point it at any OpenAI-compatible endpoint and let it rip.

Inspired by [Mihail Eric's "The Emperor Has No Clothes"](https://www.mihaileric.com/The-Emperor-Has-No-Clothes/) -- the idea that a functional coding agent is just a tight loop over an LLM with a handful of tools. **STRAP IN** is that loop, written in Rust, optimized for speed.

---

## What it does

You type a request. The LLM reads your code, edits files, runs commands, and loops until the job is done. Tokens stream to your terminal in real-time. That's it.

```
> add error handling to the database module

[tool: read_file] 2847 chars
[tool: grep] 412 chars
[tool: edit_file] 1203 chars
[tool: bash] 94 chars

Done. Added Result<T, DbError> return types to all public functions
in src/db.rs and updated the 3 call sites in src/main.rs. Tests pass.
```

## Why Rust

- **Streaming SSE** -- tokens print as they arrive, not after the full response
- **Parallel tool execution** -- multiple tool calls in one turn run concurrently via `tokio::spawn`
- **Connection pooling + TCP_NODELAY** -- minimal network latency
- **~400 lines of code** -- no framework, no SDK, just `reqwest` + `serde`
- **Tiny release binary** -- LTO, single codegen unit, stripped symbols

## Quick start

```bash
# Set your endpoint (any OpenAI-compatible API)
export STRAPIN_API_KEY="sk-..."
export STRAPIN_API_URL="https://api.openai.com/v1"
export STRAPIN_MODEL="gpt-4o"

# Build and run
cargo run --release
```

Works out of the box with:

| Provider | `STRAPIN_API_URL` |
|---|---|
| OpenAI | `https://api.openai.com/v1` |
| Ollama | `http://localhost:11434/v1` |
| vLLM | `http://localhost:8000/v1` |
| LiteLLM | `http://localhost:4000/v1` |
| OpenRouter | `https://openrouter.ai/api/v1` |
| Any OpenAI-compatible | Just set the base URL |

## Configuration

All config is via environment variables:

| Variable | Default | Description |
|---|---|---|
| `STRAPIN_API_KEY` | -- | API key (falls back to `OPENAI_API_KEY`) |
| `STRAPIN_API_URL` | `https://api.openai.com/v1` | Base URL (falls back to `OPENAI_BASE_URL`) |
| `STRAPIN_MODEL` | `gpt-4o` | Model name to use |
| `STRAPIN_WORKDIR` | Current directory | Working directory for file operations |

## Tools

The agent has 5 tools -- the core set that makes a coding agent work:

| Tool | What it does |
|---|---|
| `read_file` | Read a file with line numbers |
| `list_dir` | List directory contents (sorted, hidden files excluded) |
| `edit_file` | Replace an exact string match, or create a new file |
| `grep` | Recursive case-insensitive search with optional glob filter |
| `bash` | Run any shell command |

## Commands

While running, you can use:

| Command | Effect |
|---|---|
| `/compact` | Trim old messages to save context window |
| `/clear` | Reset the session completely |
| `quit` / `exit` | Exit |

## Architecture

```
src/
  main.rs       Entry point, REPL, config
  types.rs      API types -- messages, tool calls, SSE deltas (serde)
  client.rs     Streaming SSE client with delta reassembly
  tools.rs      Tool definitions + async executors
  agent.rs      Agent loop with parallel dispatch + context compaction
```

The agent loop is simple:

1. Send messages to the LLM (streaming)
2. If the response has tool calls, execute them all in parallel
3. Append results and go to 1
4. If no tool calls, print the response and wait for the next user input

## Tests

113 tests across unit and integration:

```bash
cargo test
```

```
running 102 tests ...  test result: ok. 102 passed
running 11 tests  ...  test result: ok. 11 passed
```

Coverage:
- **types** -- serialization, deserialization, serde renames, round-trips
- **client** -- SSE line parsing, delta accumulation, stream assembly
- **tools** -- all 5 tools with edge cases (missing params, truncation, unicode, multi-match errors)
- **agent** -- constructor, compaction boundaries, message preservation
- **integration** -- multi-step workflows, project scaffolding, shell pipelines, error recovery

## CI

GitHub Actions runs on every push to `main` and every PR:

- `cargo check`
- `cargo test`
- `cargo fmt --check`
- `cargo clippy -D warnings`
- `cargo build --release`

## License

MIT
