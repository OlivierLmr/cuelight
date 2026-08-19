# Harness

Deterministic discrete-event simulator. The harness is the network: student nodes are separate
processes speaking JSON lines on stdin/stdout.

## Why determinism and logical time

- **Logical time** lets us simulate GST (the moment message delays become bounded), which is the
  entire content of labs 2 and 3, and makes "no decision by logical time T ⇒ hung" an exact test
  rather than a flaky one. It also makes runs take milliseconds, so seed sweeps are practical.
- **Determinism** means a failure is a replayable artefact, not "it sometimes hangs".

Both were the reason for not using Jepsen's Maelstrom, which has neither. See `docs/design.md`.

## Planned module layout

| Module | Responsibility |
|---|---|
| `main.rs` | CLI |
| `proto.rs` | wire messages: `init`, `timer`, `set_timer`, `done`, observable events |
| `sim.rs` | discrete-event loop, logical clock, PRNG tie-breaking on simultaneous events |
| `net.rs` | per-link delay, GST, partitions, optional per-link FIFO mode |
| `node.rs` | process supervision: spawn, stdio framing, `done` quiescence barrier |
| `scenario.rs` | scenario format; deterministic `seed → scenario` expansion |
| `faults.rs` | crash / pause / delay / partition injection |
| `journal.rs` | totally ordered event log; determinism self-check |
| `viz.rs` | Mermaid `sequenceDiagram` emit |
| `checkers/` | per-lab property checkers |
| `oracles/` | instrumented RB / Ω / synchronizer, standing in for lower layers |

## Protocol

Maelstrom's envelope, plus three additive message types:

```
node → harness   {"type":"set_timer","after":N,"timer_id":K}
harness → node   {"type":"timer","timer_id":K}
node → harness   {"type":"done"}
```

`done` is mandatory: the simulator must know a node has finished reacting before advancing logical
time. The harness only interprets messages addressed to `"harness"`; anything addressed to a node
is opaque payload it merely routes.
