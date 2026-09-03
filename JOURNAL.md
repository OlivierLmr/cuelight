# The journal — the harness's public interface

The harness executes a scenario and writes a journal. It has **no notion of success**: it does not
know what a property is, what a critical section is, or that labs exist. Everything that judges a
run reads this file.

That makes the journal a contract, not an implementation detail. It is what a checker parses, what
`viz` renders, and what `check` diffs to prove a run is reproducible.

## Where it is

Every run writes a directory (`--out`, default `store/latest`):

| File | Contents |
|---|---|
| `journal.jsonl` | this format — one JSON object per line, in `seq` order |
| `scenario.json` | the scenario exactly as replayed, including the faults it scheduled |
| `n<i>.stderr` | each node's standard error, untouched |

A checker generally needs both `journal.jsonl` and `scenario.json`: the journal says what happened,
the scenario says what was *supposed* to happen — which nodes existed, when the network was allowed
to misbehave, and when it stopped.

## Every line

Two shapes. Both always carry `seq`, `t` and `kind`.

```json
{"seq": 5, "t": 667, "kind": "send", "src": "n0", "dest": "n1", "body": { ... }}
{"seq": 14, "t": 672, "kind": "fault-crash", "detail": {"node": "n0"}}
```

| Field | Meaning |
|---|---|
| `seq` | the harness's own ordering, dense from 0. Independent of scheduling — two events at the same `t` are still totally ordered |
| `t` | **logical** time. Never wall-clock. Non-decreasing along `seq` |
| `kind` | which of the entries below |
| `src`, `dest`, `body` | message-shaped entries: an envelope exactly as it travelled |
| `detail` | event-shaped entries: what the harness did on its own |

Nothing in a journal may vary between two runs of the same scenario against the same program: no
wall-clock, no pids, no absolute paths. That is what makes `check` meaningful.

## Message-shaped entries

| `kind` | Emitted when | Note |
|---|---|---|
| `init` | once per node, at `t=0` | `body` carries `node_id`, `node_ids`, `n`, `f`, `provided` |
| `stimulus` | the harness pokes a node | `body` comes from the lab's stimulus template; the harness never interprets it |
| `send` | a node emits a message to another node | logged when sent, not when delivered |
| `recv` | that message is delivered | absent if it was dropped or the run ended first |
| `observe` | a node reports something to the harness | **anything** addressed to `harness` that is not `set_timer` or `done` |
| `set_timer` | a node arms a timer | `body` carries `after` (logical) and `timer_id` |
| `timer` | that timer fires | |

`observe` is where properties are read from. The harness does not know or care what an observation
means — `deliver`, `enter_cs`, `leader`, or anything a future lab invents. It records the body
verbatim and moves on.

`done` is **not** journalled. It is the barrier that lets logical time advance, not an event.

## Event-shaped entries

| `kind` | `detail` | Meaning |
|---|---|---|
| `fault-crash` | `node` | that node was killed and never returns |
| `fault-pause` | `node`, `until` | stopped being scheduled until `until` |
| `fault-partition` | `side`, `until` | the network split; `side` lists one half |
| `drop-to-crashed` | `dest` | a message was discarded because its target was dead |
| `drop-from-crashed` | `src`, `dest` | a still-in-flight message was discarded because its **sender** died |
| `node-died` | `node` | a node exited **on its own**. The harness did not do this — the run has failed |
| `unknown-destination` | `dest` | a node addressed something that is not a node or `harness` |
| `time-limit` | `limit` | the run was cut off rather than settling |
| `end` | `reason`, `scheduled` | always last. `reason` is `quiescent`, `time-limit`, or a failure |

`drop-from-crashed` deserves attention when writing a checker: it is what makes a *partial
broadcast* expressible. Without it, a sender dying halfway through still delivers to everyone and
best-effort broadcast looks reliable.

## Two rules for anyone reading this file

**Date liveness from the last fault, never from `gst`.** `scenario.json` carries a scheduled `gst`,
but a pause landing after it also violates partial synchrony. The effective GST is
`max(gst, end of the last fault)` — the end of a `Pause` or `Partition` is `at + duration`, of a
`Crash` its `at`. A deadline dated from `gst` fails correct implementations.

**State properties over the nodes that never crashed.** Take `nodes` from `scenario.json`, remove
everyone with a `fault-crash` entry. A crashed node owes nothing.

## Reading it

```sh
# what kinds does a run contain?
grep -o '"kind":"[a-z-]*"' store/latest/journal.jsonl | sort | uniq -c

# what did the nodes report?
grep '"kind":"observe"' store/latest/journal.jsonl

# a sequence diagram of the messages
sdr-harness viz --journal store/latest/journal.jsonl
```

`labs/sdrcheck` is a worked reader: it loads a run directory into observations, stimuli and the
crashed set, and computes the effective GST. A checker in any other language has only to do the
same.
