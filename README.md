# Templates

Starter kits. Each provides `on(type)` handler registration, `send` / `broadcast` / `set_timer` /
`observe`, and emits `done` after every event.

All four are verified against the same scenarios and produce identical results:

| Language | Build | Deps |
|---|---|---|
| Python | — | none |
| Go | `go build ./...` | none (stdlib `encoding/json`) |
| Java | `javac -d out sdr/*.java example/Main.java` | none — a minimal JSON reader is included so no build tooling is needed |
| C++ | `c++ -std=c++17 -O2 -o node example/main.cpp` | none, header-only |

Each ships an `example/` node implementing reliable broadcast, which is both a worked example and
the template's own test:

```sh
sdr-harness run --scenario labs/lab1/scenarios/rb-crash-midsend.json --bin <your command>
```

## Three rules the templates enforce or assume

1. **`done` is emitted last, after every event.** The barrier that lets logical time advance.
2. **Your process must never exit.** An unscheduled exit fails the run: the harness knows which
   crashes it injected, and a program that dies on unexpected input must not pass as *correct under
   f=1 crashes*.
3. **A node is a pure event handler** — same event sequence in, same actions out. No wall-clock, no
   threads, no unseeded randomness. `sdr-harness check` runs a scenario twice and reports any
   difference as your bug.

## Timers

`set_timer(after, callback)` takes a **per-timer callback**, not one global handler. Ω holds a timer
per peer, the synchronizer one per view, the mutex one for the critical section — they must not
collide.

Timers **cannot be cancelled**. If you re-arm one, guard the callback with a generation counter and
ignore stale firings:

```python
def _arm(self, p):
    self._gen[p] += 1
    g = self._gen[p]
    self.node.set_timer(self.timeout[p], lambda: self._expire(p, g))

def _expire(self, p, g):
    if g != self._gen[p]:
        return          # a newer timer superseded this one
    ...
```

## Deliberately unlike Maelstrom's templates

No threads, no blocking RPC, no wall-clock. Maelstrom's are built around `resp = await rpc(dest)`,
which cannot work here: the reply arrives in a later event. Every lab in this course is one-way
messaging, so the template is smaller and the algorithm is all that is left.
