# Benchmarks

Reproducible local performance measurements for `academic-agent` v0.4.0 measured on Windows using the built release binary and isolated temporary workspaces.

## Test Environment

- OS: Microsoft Windows 11 Home Single Language (`x86_64`)
- CPU: 11th Gen Intel(R) Core(TM) i5-11400H @ 2.70GHz
- RAM: 15.7 GB
- Rust toolchain: `rustc 1.100.0 (0ef1bc386 2026-11-09)`
- Build profile: `cargo build --release --locked --bin academic-agent` (LTO fat, `codegen-units = 1`, stripped)

## Measured Results (v0.4.0)

| Benchmark | Measured Metric | Memory (Peak Working Set) | Notes |
| --- | --- | --- | --- |
| Release binary size | `9,572,352` bytes (`9.13 MB`) | N/A | Single static `.exe`, zero runtime DLL dependencies beyond OS |
| CLI cold startup (`--version`) | `21.6 ms` | `3.78 MB` | 5-run arithmetic mean |
| Config validation (`validate`) | `22.1 ms` | `4.31 MB` | 5-run arithmetic mean |
| Single document scan (`scan`) | `52.6 ms` | `12.44 MB` | Heuristic extraction + `redb` commit on 1 course syllabus |
| Duplicate document skip (`scan`) | `35.4 ms` | `3.57 MB` | Content SHA-256 lookup avoids redundant parsing |
| Batch HTTP ingestion (`POST /api/upload`) | `227.3 ms` (`220 docs/s`) | `15.37 MB` | 50 concurrent syllabus uploads into running `serve` process |
| Live `.ics` subscription feed (`GET /api/calendar/feed.ics`) | `8.59 ms` | `15.37 MB` | 50 tasks (`45,653` bytes RFC 5545 output), 5-run mean |
| CLI `.ics` export (`calendar-feed`) | `37.7 ms` | `11.27 MB` | 50 tasks exported cleanly from embedded `redb` |
| MCP `stdio` handshake (`initialize`) | `20.9 ms` | Included in stdio session | JSON-RPC initialize + initialized round-trip |
| MCP `stdio` `list_tasks` (50 tasks) | `4.1 ms` | Included in stdio session | Warm MCP `tools/call` |
| MCP `stdio` `academic://calendar/feed` (50 tasks) | `1.7 ms` | Included in stdio session | Warm MCP `resources/read` |
| LMS `.ics` batch sync (`POST /api/lms/sync`) | `463.6 ms` (`431.4 events/s`) | `12.54 MB` | 200 calendar events ingested in a single atomic `redb` transaction |
| LMS cached poll (`ETag` `304 Not Modified`) | `10.4 ms` | `12.54 MB` | 5-run mean conditional HTTP check with zero database writes |

## v0.3.0 vs v0.4.0 Comparison

Precompiled `std::sync::OnceLock<Regex>` extractors and single-transaction `redb` batch upserts reduced parsing latency and peak memory across core workflows:

| Operation | v0.3.0 | v0.4.0 |
| --- | --- | --- |
| Config validation (`validate`, 5-run avg) | `26.8 ms` | `22.1 ms` |
| Single document scan | `63.9 ms` | `52.6 ms` |
| Duplicate document skip | `36.2 ms` (`11.36 MB` peak) | `35.4 ms` (`3.57 MB` peak) |
| HTTP batch uploads (`50` docs) | `230.9 ms` / `216.5 docs/s` (`18.44 MB` peak) | `227.3 ms` / `220.0 docs/s` (`15.37 MB` peak) |
| CLI `calendar-feed` (`50` tasks) | `52.2 ms` | `37.7 ms` |

## Notes on Reproduction

- All measurements used `--no-llm` / local heuristic extraction and mock local HTTP servers to isolate Rust binary performance from external LLM or campus portal network latency.
- LMS batch sync and `304 Not Modified` polling were measured against a local HTTP fixture serving an RFC 5545 `.ics` feed with 200 distinct course events and `ETag` headers.
