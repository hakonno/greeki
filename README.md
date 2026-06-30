# spotwatt

A power-price-aware compute scheduler for a homelab. It defers deferrable work —
backups, video transcoding, model inference/training — to the cheapest hours of
the day using Norwegian spot prices (Nord Pool via
[hvakosterstrommen.no](https://www.hvakosterstrommen.no)).

You give it a shell command, an estimate of how long it runs, and (optionally) a
deadline and an estimated power draw. It figures out *when* to run it so you pay
as little as possible, and runs it for you.

## Why

A 24/7 box idles at 40–80 W no matter what. The savings aren't in turning it off
— they're in moving the *heavy, deferrable* work off the expensive price peaks
and onto the cheap night/weekend hours. Smart-home tooling does this for water
heaters and EVs; spotwatt does it for compute.

## How it works

```
hvakosterstrommen.no ──> price fetcher ──> in-memory price curve
                                                │
   jobs (sqlite) ──> scheduler tick (every 60s) ┘
                          │  asks core::plan() per job, given current prices
                          ▼
                     launches due jobs ──> executor (sh -c) ──> records result + cost
                          ▲
   dashboard (axum + maud + htmx) ── add / cancel / run-now / inspect
```

The key idea: **nothing is committed far in advance.** The price API only knows
~24–48h ahead (tomorrow's prices publish around 13:00), so every tick re-runs the
plan against the latest known prices. A job "waits" simply by not being launched
yet; when a cheaper hour arrives or new prices reveal a better window, the next
tick picks it up. Deadlines force a best-effort start when time runs out.

## Scheduling policies

- **Cheapest window** — find the cheapest contiguous block of hours long enough
  to fit the job, optionally finishing before a deadline. The workhorse.
- **Below threshold** — run as soon as the price drops to/below a kr/kWh limit.
- **Immediate** — run now regardless of price (for things you can't defer).

Priority (low/normal/high/critical) breaks ties when the concurrency cap forces a
choice; deadlines break ties after that.

## Project layout

| Path | What |
|------|------|
| `crates/core` | Pure, I/O-free scheduling logic + unit tests. `plan()` is the brain. |
| `crates/server` | The daemon: price fetch, sqlite, scheduler loop, executor, dashboard. |

Tech: Rust, [axum](https://github.com/tokio-rs/axum) + [maud](https://maud.lambda.xyz)
+ [htmx](https://htmx.org) (no JS build step), [sqlx](https://github.com/launchbadge/sqlx)
on SQLite, and the [`strompris`](https://crates.io/crates/strompris) crate for prices.

## Running

```sh
cp config.example.toml config.toml   # optional; sensible defaults otherwise
cargo run -p spotwatt-server
# open http://127.0.0.1:8080
```

Configuration is `config.toml` (path overridable via `SPOTWATT_CONFIG`), with
`SPOTWATT_REGION` and `SPOTWATT_LISTEN` env overrides. See `config.example.toml`.

### Example jobs

- **Nightly backup, must finish by 07:00**
  Policy: cheapest window · duration 45 min · deadline 07:00 · power 60 W
- **Transcode queue when power is cheap**
  Policy: below threshold · threshold 0.30 kr · power 120 W
- **Critical re-index now**
  Policy: immediate

## Tests

```sh
cargo test -p spotwatt-core      # the scheduling algorithm
```

## Status

MVP. Single-shot jobs (recurring schedules are a planned next step). The command
runs via `sh -c`; run only commands you trust.
