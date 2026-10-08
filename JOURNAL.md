# The journal: the harness's public interface

The harness executes a scenario and writes a journal. It has **no notion of success**: it does not
know what a property is, or what any of your message types mean. Everything that judges a
run reads this file.

That makes the journal a contract, not an implementation detail. It is what a checker parses, what
`viz` renders, and what `check` diffs to prove a run is reproducible.

## Where it is

Every run writes a directory (`--out`, default `store/latest`):

| File | Contents |
|---|---|
| `journal.jsonl` | this format: one JSON object per line, in `seq` order |
| `scenario.json` | the scenario exactly as replayed, including the faults it scheduled |
| `n<i>.stderr` | each node's standard error, untouched |

A checker generally needs both `journal.jsonl` and `scenario.json`: the journal says what happened,
the scenario says what was *supposed* to happen: which nodes existed, when the network was allowed
to misbehave, and when it stopped.

## Every line

Two shapes. Both always carry `seq`, `t` and `kind`.

```json
{"seq": 5, "t": 667, "kind": "send", "src": "n0", "dest": "n1", "mid": 12, "body": { ... }}
{"seq": 14, "t": 672, "kind": "fault-crash", "detail": {"node": "n0"}}
```

| Field | Meaning |
|---|---|
| `seq` | the harness's own ordering, dense from 0. Independent of scheduling, so two events at the same `t` are still totally ordered |
| `t` | **logical** time. Never wall-clock. Non-decreasing along `seq` |
| `kind` | which of the entries below |
| `src`, `dest`, `body` | message-shaped entries: the envelope as it travelled, its object keys sorted |
| `mid` | **which message this is**: dense from 1, in the order messages leave their senders. Present on every entry about a message between two nodes — `send`, `recv`, `never-sent`, `drop-to-crashed` — and absent on everything else |
| `detail` | event-shaped entries: what the harness did on its own |

Nothing in a journal may vary between two runs of the same scenario against the same program: no
wall-clock, no pids, no absolute paths. That is what makes `check` meaningful.

**Object keys are sorted, at every depth**, in `body` and in `detail`. Key order carries no meaning
in JSON and language runtimes disagree about it, so the harness writes one form: two nodes that
behaved identically produce identical bytes whatever language they were written in. Array order is
left alone, being meaningful.

## Message-shaped entries

| `kind` | Emitted when | Note |
|---|---|---|
| `init` | once per node, at `t=0` | `body` carries `node_id`, `node_ids`, `n`, `f`, `provided` |
| `stimulus` | the harness pokes a node | `body` comes from the caller's stimulus template; the harness never interprets it |
| `send` | a node emits a message to another node | logged when sent, not when delivered |
| `recv` | that message is delivered | absent if it was dropped or the run ended first |
| `observe` | a node reports something to the harness | **anything** addressed to `harness` that is not `set_timer` or `done` |
| `set_timer` | a node arms a timer | `body` carries `after` (logical) and `timer_id` |
| `timer` | that timer fires | |

`observe` is where properties are read from. The harness does not know or care what an observation
means, whether `deliver`, `enter_cs`, `leader`, or anything a caller invents. It records the body
verbatim and moves on.

`done` is **not** journalled. It is the barrier that lets logical time advance, not an event.

## Event-shaped entries

| `kind` | `detail` | Meaning |
|---|---|---|
| `fault-crash` | `node` | that node was killed and never returns |
| `fault-pause` | `node`, `until` | stopped being scheduled until `until` |
| `fault-partition` | `side`, `until` | the network split; `side` lists one half |
| `drop-to-crashed` | `src`, `dest`, `mid` | a message reached a node that was already dead, and died at its lifeline |
| `never-sent` | `src`, `dest`, `mid` | a message its author never got out: it died partway through its send loop |
| `node-died` | `node` | a node exited **on its own**. The harness did not do this, and the run has failed |
| `unknown-destination` | `dest` | a node addressed something that is not a node or `harness` |
| `time-limit` | `limit` | the run was cut off rather than settling |
| `end` | `reason`, `scheduled` | always last. `reason` is `quiescent`, `time-limit`, or a failure |

`never-sent` deserves attention when writing a checker: it is what makes a *partial broadcast*
expressible. Without it, a sender dying halfway through still delivers to everyone and best-effort
broadcast looks reliable.

The name says what happened rather than how it was implemented: the message did not leave, so
nothing was dropped in flight, because there was never anything in flight. This entry was
documented here as `drop-from-crashed` for a while — a name no version of the harness ever wrote.

## Pairing a `recv` with its `send`

Use `mid`, and nothing else.

It is tempting to match oldest-unmatched-first per link. That is exactly right when the link is
FIFO and wrong otherwise — and `fifo` is a scenario field meant to be turned off, with
`jitter_pct` defaulting to 100 so reordering genuinely happens. A mispairing is invisible in a list
and obvious in a drawing: it puts two arrows crossing that never crossed.

## Two rules for anyone reading this file

**Date liveness from the last fault, never from `gst`.** `scenario.json` carries a scheduled `gst`,
but a pause landing after it also violates partial synchrony. The effective GST is
`max(gst, end of the last fault)`. The end of a `Pause` or `Partition` is `at + duration`, of a
`Crash` its `at`. A deadline dated from `gst` fails correct implementations.

**State properties over the nodes that never crashed.** Take `nodes` from `scenario.json`, remove
everyone with a `fault-crash` entry. A crashed node owes nothing.

## Reading it

```sh
# what kinds does a run contain?
grep -o '"kind":"[a-z-]*"' store/latest/journal.jsonl | sort | uniq -c

# what did the nodes report?
grep '"kind":"observe"' store/latest/journal.jsonl

# a sequence diagram: horizontal arrows, renders natively on GitHub
cuelight viz --journal store/latest/journal.jsonl

# a space-time diagram: arrows slant from the instant a message left to the instant it landed
cuelight viz --journal store/latest/journal.jsonl --format cuesheet
cuesheet render store/latest/messages.cuesheet --style cuesheet.cuestyle --out run.svg
```

A checker has only to load the run directory into observations, stimuli, the messages nodes sent
one another and the crashed set, and compute the effective GST from the scenario. Nothing else is
required of it. `cuelight-suite` does that loading: its `Events` carries each `send` entry as
instant, sender, destination, `mid` and body, in journal order, so a property about traffic never
has to find this file itself.
