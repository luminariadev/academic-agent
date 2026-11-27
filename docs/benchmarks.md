# Benchmarks

This document records reproducible latency, throughput, and working-set memory measurements for `academic-agent` v0.3.0.

## Test Environment

- **OS**: Microsoft Windows 11 Home Single Language (x86_64)
- **CPU**: 11th Gen Intel Core i5-11400H @ 2.70 GHz
- **Memory**: 15.7 GB RAM
- **Toolchain**: `rustc 1.100.0 (0ef1bc386 2026-11-09)`, release profile (`opt-level = 3`, `lto = true`, `codegen-units = 1`, `strip = true`)
- **Binary size**: `academic-agent.exe` = 8.67 MB (9,095,680 bytes)

## Methodology

1. CLI wall-clock timings and `PeakWorkingSet64` memory were captured externally using `.NET` `System.Diagnostics.Stopwatch` and `System.Diagnostics.Process` around `target\release\academic-agent.exe` using temporary TOML configurations and `redb` databases.
2. Startup (`--version`) ran one unmeasured OS cache warmup followed by a 5-run average; config validation (`validate`) ran a 5-run average.
3. Batch HTTP ingestion launched `academic-agent serve` bound to `127.0.0.1` and uploaded 50 distinct bilingual assignment documents across 8 concurrent workers via `POST /api/upload?heuristic=true&notify=false`.
4. Live iCalendar feed latency measured `GET /api/calendar/feed.ics` over the 50-task store (5-run average) and compared against `academic-agent calendar-feed --output`.
5. MCP latency was measured over an active `academic-agent mcp` `stdio` process sending newline-delimited JSON-RPC 2.0 requests against the 50-task store.

## Results

| Scenario | Command / Operation | Wall Time | Throughput / Size | Peak RAM |
| :--- | :--- | :--- | :--- | :--- |
| Binary startup | `academic-agent --version` (5-run average) | 14.5 ms | n/a | 3.42 MB |
| Config validation | `academic-agent --config examples/academic-agent.toml validate` (5-run avg) | 26.8 ms | n/a | 6.82 MB |
| Single scan + `.ics` | `academic-agent scan doc.txt --heuristic` (`redb` insert) | 63.9 ms | 1 event written | 11.46 MB |
| Duplicate skip | Repeat `academic-agent scan doc.txt --heuristic` (SHA-256 hit) | 36.2 ms | 0 writes | 11.36 MB |
| Batch 50 uploads | `POST /api/upload?heuristic=true&notify=false` (8-way concurrency) | 230.9 ms | 216.5 docs/s | 18.44 MB (`serve`) |
| Live `.ics` HTTP feed | `GET /api/calendar/feed.ics` (50 events, 5-run average) | 7.96 ms | 45,653 bytes | shared (`18.44 MB`) |
| CLI `.ics` feed export | `academic-agent calendar-feed --output feed.ics` (50 events) | 52.2 ms | 50 events | 11.16 MB |
| MCP `initialize` | JSON-RPC `initialize` over `stdio` | 9.1 ms | n/a | n/a |
| MCP `list_tasks` | `tools/call list_tasks` across 50 stored tasks | 1.4 ms | 50 tasks | n/a |
| MCP feed resource | `resources/read academic://calendar/feed` across 50 events | 1.1 ms | 50 events | n/a |

## Observations

- Processing 50 concurrent document uploads (text extraction, heuristic parsing, `.ics` disk generation, and transactional `redb` indexing) finishes in 230.9 ms while holding server peak working set memory at 18.44 MB.
- Repeat ingestion of identical files short-circuits via SHA-256 hash lookup in `redb` (`36.2 ms` total process lifetime vs `63.9 ms` for new writes) without duplicating calendar artifacts or notifications.
- Generating a complete 50-event RFC 5545 subscription feed runs in 7.96 ms over HTTP (`45.65 KB`) and 1.1 ms on an active `stdio` MCP session.
