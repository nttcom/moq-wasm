# `moqt` Quint models

Quint models of the state machines listed in [`../state-machines.md`](../state-machines.md). Each file models one machine (or a tightly coupled group) of `crates/moqt`; its header names the code it follows and what one action stands for (one lock hold, or one stretch of a task between two `.await`s).

## Conventions
- The first module of a file is the parameterized model; the modules after it are instances.
- `<model>_current` models today's code. Switches (`const FIX_*`, `const *: bool`) stand for a code change; `<model>_fixed` turns on the smallest set that makes `safety` hold.
- Other instances turn off one guard the code relies on, or one fix, and must fail: they show the invariant is not vacuous and which switch it depends on.
- Instances are small (2 actors, ids 0..3) so that TLC explores **every** reachable state.

## Running
Needs Node and Java 17+ (`quint verify` downloads Apalache, which translates the model for TLC).

```sh
docs/architecture/moqt/models/check.sh                    # every instance, TLC, compared with the expectation
docs/architecture/moqt/models/check.sh migration.qnt      # one file
npx @informalsystems/quint@0.34.0 verify --backend tlc --main migration_current --invariant safety migration.qnt
npx @informalsystems/quint@0.34.0 run --main migration_current --invariant safety migration.qnt   # random simulation, prints a trace
```

Each instance module is preceded by `// expect: ok` or `// expect: violation`; `check.sh` compares the TLC outcome of `safety` with it. Run instances one at a time: the TLC backend starts an Apalache server on a fixed port, so concurrent runs collide.

To see a counterexample, run `verify` without `--backend tlc` (Apalache, bounded by `--max-steps`) or `run` with the same `--main`; both print the trace.
