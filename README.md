# spotwatt

A power-price-aware compute scheduler for a homelab. It moves heavy,
non-urgent work — backups, video transcoding, model training — to the cheapest
hours of the day using Norwegian spot prices (Nord Pool via
[hvakosterstrommen.no](https://www.hvakosterstrommen.no)).

If you just want the architecture, jump to [How it fits together](#how-it-fits-together).
Otherwise, here's the whole idea in plain terms first.

---

## The problem

You have a server that runs around the clock. Electricity costs a different
amount every hour (the spot price). At night and in the middle of the day it's
often cheap; in the morning and evening it's expensive. The gap can be large.

A lot of what the server does isn't urgent. A backup, a video conversion, a
training run for an AI model — it doesn't matter whether it runs at 14:00 or at
03:00, as long as it finishes. But most people kick off jobs like that the
moment they think of them, and pay the expensive price for no reason.

## The idea

You stop saying *"run this job now."* You say *"run this job when electricity is
cheapest, but it has to be done before 07:00."*

The program looks at the prices, finds the cheapest window, and starts the job
itself at the right moment. You don't have to think about it.

## The principle (the important part)

There's a catch: the price list only goes 24–48 hours ahead. You don't know what
electricity costs on Thursday when it's Monday (tomorrow's prices publish around
13:00).

So the program does **not** build a fixed plan far into the future. Instead it
asks itself one question, over and over, once a minute:

> *"Based on the prices I know right now, is this hour the right time to start
> the job?"*

If the answer is no, it does nothing and asks again in a minute. If a cheaper
hour shows up, or tomorrow's prices arrive with a better window, that's caught
automatically the next time it asks. If the deadline is getting close and time
is running out, it starts anyway.

That's the whole trick. Instead of deciding once, it makes the same small
decision continuously — so it never has to guess about the future.

## How it fits together

Think of it as four parts talking to each other:

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

1. **The fetcher** asks hvakosterstrommen.no every 30 minutes and keeps a fresh
   price curve in memory.
2. **The brain** (`core::plan()`) is a small function that takes the prices, the
   job, and the clock, and answers *"run now"* or *"wait until then."* It's
   completely isolated and does nothing else, so it's easy to test — that's the
   part with 9 unit tests.
3. **The scheduler** wakes the brain every 60 seconds for each pending job and
   launches the ones that are due.
4. **The executor** runs the actual command, captures its output, whether it
   succeeded, and what it actually cost in kroner.

On top of it all sits a web dashboard where you add jobs and watch status; it
refreshes itself every few seconds.

## Scheduling policies

You pick one of three ways for a job to wait:

- **Cheapest window** — find the cheapest contiguous block of hours long enough
  to fit the job, optionally finishing before a deadline. The workhorse, for
  backups, conversions, training.
- **Below threshold** — run as soon as the price drops to/below a kr/kWh limit.
  Simple and predictable.
- **Immediate** — run now regardless of price, for things you can't defer.

Priority (low/normal/high/critical) breaks ties when the concurrency cap forces
a choice; deadlines break ties after that.

## Why these choices

- **Rust** — the server runs 24/7, so you want something that uses little
  memory, doesn't crash, and is a single binary you just start.
- **Server-rendered web** (not a heavy in-browser app) — for a small dashboard
  you glance at now and then, server-side HTML is simpler, faster, and plenty.
  No extra build step.
- **One pure, isolated function for the decision** — when "run or wait" lives
  entirely on its own, I can test that it's correct without starting the whole
  system. That's where a bug would do the most damage, so that's where I wanted
  certainty.

## What it actually does for you

It moves the heavy, non-urgent jobs off the expensive hours and onto the cheap
ones, automatically. The server draws the same idle power either way — the win
isn't in switching it off. It's in letting it work when electricity is cheap
instead of when it's expensive.

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
