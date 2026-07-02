# spotwatt — critique and direction

A candid review of what spotwatt is, where it earns its keep, where it doesn't,
and what it would take to make it genuinely useful. The goal is not to talk the
project down — the engineering is solid — but to point the same machinery at a
problem big enough to be worth solving.

## What's good

- **The decision is a pure, isolated function.** `core::plan()` takes the price
  series, the job spec, and `now`, and returns "run or wait." No I/O, no clock,
  no database. That's exactly where the risk concentrates, and it's exactly
  where the tests are.
- **Re-decide every tick, never commit to a far-future plan.** The spot API only
  knows ~24–48h ahead. Rather than building a brittle schedule, every tick
  re-runs `plan()` against the latest prices. A job "waits" by simply not being
  told to run yet. This is the right shape for the problem.
- **Correct concurrency claim.** `claim_for_running` flips `pending → running`
  atomically, so a slow executor start can't double-launch a job.
- **Honest README.** It states the awkward truth — "the server draws the same
  idle power either way" — instead of hiding it.

## The core problem: it optimizes the wrong number, at a scale where the answer barely matters

### 1. At homelab compute scale, the savings are rounding error

Because the machine is on 24/7 regardless, the only money moved is the
*marginal* energy of the job itself:

| Job | Energy | × peak-vs-cheap spread | Saving |
|-----|--------|------------------------|--------|
| Nightly backup, 60 W, 45 min | 0.045 kWh | ~1 kr/kWh (generous) | **~0.05 kr/night ≈ 16 kr/year** |
| Heavy transcode, 120 W, 4 h, daily | 0.48 kWh | ~1 kr/kWh | **~0.48 kr/day ≈ 175 kr/year** |

That is the *ceiling*, before the next two points shrink it further. No homelab
backup justifies a scheduler on energy savings alone.

### 2. Raw spot price is not what a Norwegian household actually pays

The real marginal price is `spot + nettleie (energy part) + elavgift + 25% MVA`,
and — decisively — **strømstøtte** refunds roughly 90% of spot above a threshold
(~0.9 kr/kWh ex-VAT; the scheme's exact threshold and cadence keep changing).
The practical effect is that the *top* of the spot curve is compressed to nearly
flat for the consumer. spotwatt chases a peak-to-trough spread the customer
largely doesn't experience, and `est_cost_nok` (raw spot × kWh) both overstates
the absolute cost and exaggerates the spread the optimizer exploits.

### 3. It ignores the one fee that actually rewards load-shifting: the capacity tariff

Since 2022, Norwegian grid bills include a **capacity component**
(kapasitetsledd / effekttariff) keyed to your *peak hourly consumption* —
typically the average of your few highest hours in the month. The lever that
saves real money is avoiding **simultaneity**, not chasing cheap kWh. spotwatt's
concurrency cap is **by job count, not by watts**, so it cannot reason about
peaks at all.

**Conclusion:** spotwatt is a well-built answer to a problem that, as scoped, is
too small and aimed at the wrong cost signal. The real use cases are one step
away, and the architecture generalizes to them cleanly.

## The real use cases

### A. Shift *large* deferrable loads, not 60 W backups
EV charging (~11 kW), water/immersion heater (~2–3 kW), home battery. A 4 kr
spread on 10 kWh is **40 kr/night**, not 0.05 kr. The `plan()` brain already does
the right thing; what's missing is an *actuator* beyond `sh -c` (smart
plugs/relays, Tibber/Easee/Zaptec/Shelly/Home Assistant).

### B. Minimize the capacity tariff — best ROI
Make concurrency **power-aware**: a configurable site power budget, track
committed watts, and refuse to start a job that would push the hour over budget.
This turns the count-based cap into a real peak-shaving controller and reuses
everything already there.

### C. Solar self-consumption
With PV, the cheapest hours are when you're producing. Offset the price curve by
a production signal so jobs prefer self-consumption.

### D. Honest savings reporting
Nothing currently tells the user whether the tool helped. Report shifted kWh and
kr saved versus running-on-submit, computed against the *effective* tariff — not
raw spot. If that number is honestly tiny for compute jobs, that validates
pivoting toward A/B.

## Correctness / robustness bugs (independent of strategy)

1. **Crash leaves jobs stuck `running` forever.** No startup reconciliation; an
   orphaned `running` row permanently consumes a concurrency slot.
2. **No execution timeout.** A hung command holds a slot indefinitely;
   `duration_minutes` is never enforced as a wall-clock cap.
3. **No price-staleness guard.** If fetch fails for a day, decisions run on an
   expired curve and a job can fire "cheap" on yesterday's prices.
4. **Open RCE.** The create endpoint runs arbitrary `sh -c` with no auth; the
   moment `listen` is `0.0.0.0` (which the config invites), anyone on the LAN has
   remote code execution.
5. **Duration is a fixed guess with no feedback.** Overruns bleed into expensive
   hours and corrupt the cost estimate.
6. **Recurring jobs don't exist.** The flagship example — "nightly backup" —
   can't actually run nightly; every job is single-shot.

## Direction taken in this branch

Reframing from "schedule compute jobs" to **a price- and peak-aware load
controller**, the highest value-per-effort work is implemented here:

1. **Effective-tariff cost model** (`core::tariff`) — converts raw spot into the
   price the consumer actually pays (grid energy, electricity tax, VAT, and
   strømstøtte refund). Scheduling and cost estimates now optimize the real
   number.
2. **Power-aware concurrency** — the scheduler enforces a site power budget (kW)
   in addition to the job count cap, so it can shave peaks instead of merely
   limiting parallelism.
3. **Robustness fixes** — orphaned `running` jobs are reconciled on startup;
   the executor enforces a per-job timeout; a stale price curve is flagged in
   the log, and price-driven policies decline to start on it (only Immediate
   and deadline-forced jobs run).
4. **Recurring jobs** — jobs can repeat on a daily cadence so the flagship
   nightly-backup use case actually works.

Remaining future work, in priority order: an `Actuator` abstraction for real
high-draw devices (A), solar self-consumption (C), a savings rollup view (D),
and learned duration estimates from run history (bug 5).
