# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

See [API_DESIGN_GUIDELINES.md](API_DESIGN_GUIDELINES.md) for CLI command naming, flag conventions, and output format rules.

## Commands

```bash
# Build
cargo build           # debug
cargo build --release # production binary (~4MB)

# Run
cargo run -- <args>   # e.g. cargo run -- ls -o json
./target/release/lox <args>

# Check / Lint
cargo check
cargo clippy

# Test
cargo test
cargo test <test_name>  # run a single test
```

## Pre-push CI Checklist

**ALWAYS run these four checks before committing/pushing.** They mirror `.github/workflows/ci.yml` (ubuntu + macos):

```bash
cargo fmt --check            # formatting
cargo clippy -- -D warnings  # lints (warnings are errors)
cargo build --release        # release build
cargo test                   # all tests
```

Do not push until all four pass. Fix issues and re-run before committing.

## Architecture

Single Rust binary. CLI commands use reqwest blocking; token auth uses tokio + WebSocket for the RSA/AES key exchange.

### Source files

| File | Purpose |
|------|---------|
| `src/main.rs` | All CLI commands (30+), clap argument parsing, helper functions (RGB→HSV, weather/stats binary parsing) |
| `src/client.rs` | `LoxClient` (HTTP) — control resolution, structure cache, categories, global states, operating modes |
| `src/config.rs` | `Config` + `GlobalConfig` — loads/saves config (flat or multi-context), context resolution, project-local `.lox/` discovery |
| `src/commands/ctx.rs` | `lox ctx` commands — add/use/list/remove/rename/init/migrate contexts |
| `src/gitops.rs` | Git-based config versioning — init, pull (FTP→LoxCC→diff→commit), log, restore workflows |
| `src/scene.rs` | Scene loading/listing from `~/.lox/scenes/*.yaml` |
| `src/ws.rs` | `LoxWsClient` — async WebSocket connection used by token auth (RSA+AES key exchange handshake) |
| `src/token.rs` | Token auth flow: RSA key exchange, AES-encrypted credential exchange, token storage, HMAC token hashing |

### Key design points

**Control resolution** (`LoxClient::resolve_with_room`): Names are matched against the structure cache using fuzzy substring matching. Resolution order: alias → exact UUID → bracket room qualifier (`"Name [Room]"`) → `--room` flag → fuzzy substring. Ambiguous matches are an error.

**Multi-context config**: `~/.lox/config.yaml` supports both flat (single-Miniserver, backward compatible) and multi-context format. `Config::load()` resolution: `LOX_CONFIG` env → project-local `.lox/` (walk up from cwd) → global config → `--ctx` flag override. Each context gets isolated data under `~/.lox/contexts/<name>/`.

**Structure cache**: `LoxApp3.json` (~150KB) is cached per-context (e.g. `~/.lox/contexts/<name>/cache/structure.json`) with a 24h TTL. All commands that need control UUIDs load this cache first; `lox cache refresh` forces a re-fetch.

**Mixed sync/async**: The CLI commands use `reqwest::blocking`. `main.rs` uses `#[tokio::main]` because `lox token fetch` needs async (WebSocket for the key exchange). The blocking reqwest client spawns its own thread pool so both modes coexist.

**Token auth**: RSA public key fetched from Miniserver → encrypt AES session key → send encrypted credentials via WebSocket → receive token. Token stored per-context (e.g. `~/.lox/contexts/<name>/token.json`), valid ~20 days.

### User data layout

```
~/.lox/
  config.yaml          # flat (single-Miniserver) or multi-context format
  contexts/            # per-context data (multi-context mode)
    <name>/
      cache/
        structure.json # LoxApp3.json cache (24h TTL)
      token.json       # optional token auth
      scenes/*.yaml    # multi-step scene definitions
  # Legacy flat mode (backward compatible):
  cache/
    structure.json     # LoxApp3.json cache (24h TTL)
  token.json           # optional token auth
  scenes/*.yaml        # multi-step scene definitions
```

Project-local config (auto-discovered by walking up from cwd):
```
project/
  .lox/
    config.yaml        # connection settings
    .gitignore         # excludes secrets and cache
    cache/
    scenes/
```

### Loxone HTTP API used

```
GET /data/LoxApp3.json              → full structure (controls, rooms, categories, globalStates)
GET /jdev/sps/io/{uuid}/{cmd}       → send command → JSON response
GET /dev/sps/io/{uuid}/all          → XML: all state outputs for a control
GET /dev/sps/io/{name}/state        → input state by name
GET /dev/sys/heap, /dev/sps/state, /dev/cfg/version, /data/status  → status info
GET /jdev/sys/lastcpu, numtasks, contextswitches, sdtest           → diagnostics
GET /jdev/sys/ints, comints, contextswitchesi                      → additional diagnostics
GET /jdev/cfg/ip, mac, mask, gateway, dns1, dns2, dhcp, ntp       → network config
GET /jdev/bus/packetssent, packetsreceived, ..., parityerrors      → CAN bus stats
GET /jdev/lan/txp, txe, txc, txu, rxp, rxo, eof, exh, nob        → LAN stats
GET /jdev/sys/date, /jdev/sys/time                                 → system clock
GET /jdev/sps/LoxAPPversion3        → structure file version check
GET /binstatisticdata/{uuid}/{period} → binary statistics (u32 ts + f64[] values)
GET /data/weatheru.bin              → binary weather data (108-byte entries)
GET /dev/fsget/{path}               → filesystem access
GET /jdev/sys/checktoken, refreshtoken, killtoken                  → token management
GET /jdev/sys/reboot                → reboot Miniserver
GET /jdev/sys/updatetolatestrelease → firmware update
WSS /ws/rfc6455                     → WebSocket for token auth key exchange
UDP :7070                           → Miniserver discovery (broadcast)
HTTP :7091/zone/{n}/{cmd}           → Music server API (unofficial)
```

TLS: `danger_accept_invalid_certs(true)` is used throughout (Miniserver uses self-signed certs). When `serial` is set in config, `Config::tls_host()` generates the DynDNS hostname for valid cert matching.

## Agent Workflow

### Non-Interactive Shell Commands

**ALWAYS use non-interactive flags** with file operations to avoid hanging on confirmation prompts.

Shell commands like `cp`, `mv`, and `rm` may be aliased to include `-i` (interactive) mode on some systems, causing the agent to hang indefinitely waiting for y/n input.

```bash
cp -f source dest    # NOT: cp source dest
mv -f source dest    # NOT: mv source dest
rm -f file           # NOT: rm file
rm -rf directory     # NOT: rm -r directory
```

Other commands that may prompt: `scp`/`ssh` — use `-o BatchMode=yes`; `apt-get` — use `-y`; `brew` — set `HOMEBREW_NO_AUTO_UPDATE=1`.

### Session Completion

**Work is NOT complete until `git push` succeeds.**

1. File issues for remaining work (`bd create`)
2. Run quality gates — `cargo test`, `cargo clippy`
3. Update issue status — close finished work
4. Push:
   ```bash
   git pull --rebase
   bd backup        # export issues to .beads/backup/ and commit
   git push
   git status       # must show "up to date with origin"
   ```
5. Verify all changes committed and pushed before ending the session

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:7510c1e2 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->
