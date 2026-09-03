# Templates

Starter kits. Each provides `on(type)` handler registration, `send` / `set_timer` / `observe`, and
emits `done` after every event.

There is deliberately **no `broadcast`**. Sending to every peer is three lines, and "to everyone
*except* me" is a design decision the student should make rather than inherit — in reliable
broadcast the sender must still deliver to itself, which the loop alone does not do. `peers` is
exposed; the loop is theirs.

All four are verified against the same scenarios and produce identical results:

| Language | Build | Deps |
|---|---|---|
| Python | — | none |
| Go | `go build ./...` | none (stdlib `encoding/json`) |
| Java | `javac -d out cuelight/*.java example/Main.java` | none — a minimal JSON reader is included so no build tooling is needed |
| C++ | `c++ -std=c++17 -O2 -o example example/main.cpp` | none, header-only |

Each ships an `example/` node playing **ping-pong**: n0 pings the next node around the ring, which
pongs back, for five rounds. It touches every part of the template — handler registration, `send`,
`set_timer` with a callback, `observe` — and deliberately implements **no algorithm from any lab**,
so it can ship anywhere without giving an answer away.

```sh
cuelight run --seed 1 --no-faults --bin <your command>
```

The four are verified to produce a byte-identical observation trace on the same scenario. That is
the template's own test: if your language's plumbing is right, you get exactly this.

```
358 n1 saw_ping 1      1469 n1 saw_ping 3      1528 n1 saw_ping 5
696 n0 saw_pong 1      1470 n0 saw_pong 3      1529 n0 saw_pong 5
985 n1 saw_ping 2      1498 n1 saw_ping 4
1200 n0 saw_pong 2     1499 n0 saw_pong 4
```

## Three rules the templates enforce or assume

1. **`done` is emitted last, after every event.** The barrier that lets logical time advance.
2. **Your process must never exit.** An unscheduled exit fails the run: the harness knows which
   crashes it injected, and a program that dies on unexpected input must not pass as *correct under
   f=1 crashes*.
3. **A node is a pure event handler** — same event sequence in, same actions out. No wall-clock, no
   threads, no unseeded randomness. `cuelight check` runs a scenario twice and reports any
   difference as your bug.

## Timers

`set_timer(after, callback)` takes a **per-timer callback**, not one global handler. Ω holds a timer
one per peer, one per round, one per pending request — they must not
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
