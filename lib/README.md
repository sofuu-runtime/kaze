# Kaze Standard Robotics Library (v1.0)

Canonical, composable Kaze modules. Every file is a normal module: `use` it,
merge its `needs:`, and select a routine with `--entry` or let its hooks run.

| Module | Provides | Needs |
| :--- | :--- | :--- |
| `lib/safety.kaze` | Low-battery and link-loss RTL hooks, both best-effort | `drone`, `log` |
| `lib/guardrail.kaze` | `filter user_prompt(prompt)` — length + secret checks | `policy` |
| `lib/survey.kaze` | `action eight_point_survey()` — arm, climb, 8 waypoints, RTL | `drone`, `log` |

## Compose a mission

```kaze
use "lib/safety.kaze"
use "lib/survey.kaze"

needs: log

action main():
    log "composed mission: select eight_point_survey with --entry"
```

```bash
kaze run mission.kaze --entry eight_point_survey \
  --cap drone --cap log --ticks 2 --tick-ms 100 --plain
```

`--entry eight_point_survey` runs the library routine; the safety hooks run on
every tick. A library routine is never the implicit entry (v0.5 entry rules).

## Guardrail

```bash
kaze run mission.kaze --entry user_prompt \
  --arg prompt='"my api_key is sk-live-123"' --json
```

The report's `verdict` is `Reject` for a leaked credential and `Allow` for a
normal prompt.

## Rules the library follows

- `needs:` is explicit in every file; the loader merges and deduplicates it.
- No file depends on another library file (no import cycles, no hidden
  ordering).
- Hooks are idempotent: a lost link can fire the RTL hook on every tick.
- The RTL in the safety hooks is wrapped in `try ... otherwise`; losing the
  link must not lose the supervisor.
- Every item keeps its file origin, so a failure names `lib/safety.kaze`, not
  the mission that imported it.
