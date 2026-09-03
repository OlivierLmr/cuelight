# cuelight

A deterministic discrete-event simulator for distributed systems.

> **On authorship.** I designed and own this project; most of the code was written by Claude under
> my direction. Full breakdown: [Human / AI split](#human--ai-split).

**cuelight is the network.** Your nodes are ordinary processes speaking JSON lines on stdin and
stdout; cuelight owns logical time, routes every message, decides what is slow, what is lost and
who dies, and writes down everything that happened.

It has **no notion of success**. It does not know what a property is. It runs a scenario and
produces a journal; whatever judges that journal is yours to write.

```sh
cargo build --release
./target/release/cuelight run --seed 1 --bin ./my-node
```

## Why it exists

Two properties, both non-negotiable, and each rules out most alternatives:

- **Logical time.** Delays, timeouts and the moment the network settles are all simulated. A run
  takes milliseconds, so sweeping two hundred seeds is a normal thing to do, and *"nothing was
  decided by time T"* is an exact statement rather than a flaky one.
- **Determinism.** The same scenario against the same program produces a byte-identical journal.
  A failure is therefore a **replayable artefact**, not "it hangs sometimes". `cuelight check`
  runs a scenario twice and diffs the journals, which catches wall-clock reads, threads and
  unseeded randomness in *your* program.

## Your side of the contract

A node is a **pure event handler**: same event sequence in, same actions out. Three rules follow,
and cuelight enforces all three.

1. **Emit `done` after every event, last.** It is the barrier that lets logical time advance.
   Without it you are declared hung.
2. **Never exit on your own.** An unscheduled exit fails the run. cuelight knows which crashes it
   injected, and a program that dies on unexpected input must not pass as surviving them.
3. **No wall-clock, no threads, no unseeded randomness.** Replay depends on it.

## The wire protocol

One JSON object per line, both directions:

```json
{"src": "n1", "dest": "n2", "body": {"type": "...", "...": "..."}}
```

cuelight interprets only messages addressed to `"harness"`. Anything addressed to a node is
**opaque payload** it merely routes: the contents of your protocol are none of its business.

**It sends you:**

| `type` | Meaning |
|---|---|
| `init` | `node_id`, `node_ids`, `n`, `f`, `provided`. Once, at t=0 |
| `timer` | a timer you armed has expired (`timer_id`) |
| *anything else* | a **stimulus** from the workload template; its shape is yours to define |

**You send it:**

| `type` | Meaning |
|---|---|
| `set_timer` | arm a timer: `after` (in **logical** time) and `timer_id` |
| `done` | **required**, last, after every event |
| *anything else* | an **observation**, recorded verbatim in the journal |

That last row is the whole extension mechanism. `deliver`, `elected`, `committed`. Invent what you
need; cuelight records it and never interprets it.

## What it can do to you

| Fault | Effect |
|---|---|
| `crash` | the node stops forever. Messages still in flight from it are dropped, which is what makes a *partial* broadcast expressible |
| `pause` | alive but processes nothing until `at + duration`. Its clock keeps running |
| `partition` | messages crossing the split are **held**, not dropped, and delivered when it heals |

Plus, on every link: a delay drawn per link, larger before **GST** and small after it; a jitter
that scales with the link's own delay so reordering is actually possible; and an optional
per-link **FIFO** mode (`--fifo`) for algorithms that require ordered channels.

GST, the *Global Stabilisation Time*, is the instant after which delays become bounded. Before
it, the network may behave arbitrarily badly. It is what makes partial synchrony testable.

## Scenarios

A scenario, not a seed, is the replay unit. A seed would be consumed differently as your program
changes, so the same seed would stop meaning the same run. A scenario pins everything cuelight
controls, so it replays against any version of your code, can be shrunk by hand, and can be pasted
into a bug report.

```json
{
  "nodes": 3, "f": 1, "gst": 2678, "time_limit": 10000, "fifo": true,
  "delay_pre":  [[0,341,205],[194,0,255],[137,214,0]],
  "delay_post": [[0,16,2],[7,0,18],[3,5,0]],
  "jitter_pct": 100,
  "faults": [
    {"kind": "crash",     "at": 672,  "node": "n0"},
    {"kind": "pause",     "at": 900,  "node": "n1", "duration": 200},
    {"kind": "partition", "at": 3043, "duration": 209, "side": ["n0"]}
  ],
  "stimuli": [{"at": 667, "node": "n0", "body": {"type": "whatever-you-want"}}]
}
```

Every field has a default, so a hand-written directed test can be as small as
`{"nodes": 4, "stimuli": [...]}`. To author one, start from a generated scenario and edit it:

```sh
cuelight scenario --seed 7 --nodes 4 --fifo > my-test.json
cuelight run --scenario my-test.json --bin ./my-node
```

## Workloads

cuelight ships **no** workload of its own: poking a node with `do_broadcast` or `propose` would
mean knowing what those mean. You supply a template, and it expands against the seed:

```json
{ "events": [ { "count": [3, 9], "at_frac": [0.0, 0.5],
                "body": { "type": "ping", "id": "m<i>" } } ] }
```

`count` draws how many events; `at` or `at_frac` when; `per_node` emits one per node instead of a
drawn count; `<i>` inside a string becomes the event index; `{"$rand": [lo, hi]}` becomes a drawn
integer. Pass it with `--stimuli`.

The draw order is part of what a seed *means*: change it and every stored seed silently starts
describing a different run.

## Reading the result

Every run writes a directory (`--out`, default `store/latest`):

| File | Contents |
|---|---|
| `journal.jsonl` | one JSON object per line: everything that happened, totally ordered |
| `scenario.json` | the scenario exactly as replayed |
| `n<i>.stderr` | each node's standard error, untouched |

**[JOURNAL.md](JOURNAL.md) is the format**, and it is a stable contract: it is what your checker
parses, what `viz` renders, and what `check` diffs.

Plugging in your own checker means nothing more than reading those two files after the run. To
sweep, loop over `run --seed i` and judge each journal. cuelight has no `sweep` of its own,
because pruning the runs that passed would mean knowing what passing is.

## Commands

```
cuelight run    [options] --bin <cmd...>    one run
cuelight check  [options] --bin <cmd...>    run twice, verify identical journals
cuelight viz    --journal <path>            Mermaid sequence diagram
cuelight scenario [options]                 print an expanded scenario, to edit by hand
```

| Option | |
|---|---|
| `--bin <cmd...>` | command launching one node. **Must be last**, it swallows the rest of the line |
| `--scenario <path>` | replay a stored scenario instead of expanding a seed |
| `--seed <s>` | seed to expand (default 1) |
| `--nodes <n>` / `--faults <f>` | node count (4) and crashes tolerated (1) |
| `--no-faults` | expand a clean run |
| `--fifo` | per-link FIFO ordering |
| `--stimuli <path>` | workload template |
| `--time-limit <t>` | logical time limit (10000) |
| `--watchdog <ms>` | wall-clock hang detector (5000) |
| `--out <dir>` | run directory (`store/latest`) |

## A complete example

`templates/*/example/` holds the same ping-pong node in Python, Go, Java and C++: n0 pings the
next node around the ring, which pongs back, five times. It exercises handler registration, `send`,
`set_timer` with a callback and `observe`, and is about forty lines.

```sh
cuelight run --seed 1 --no-faults --bin python3 templates/python/example/pingpong.py
grep '"kind":"observe"' store/latest/journal.jsonl
```

```
358 n1 saw_ping 1     985 n1 saw_ping 2    1469 n1 saw_ping 3
696 n0 saw_pong 1    1200 n0 saw_pong 2    1470 n0 saw_pong 3
```

All four languages produce that trace character for character. If yours does too, your plumbing is
right.

`templates/` also holds the node runtime for those four languages: the event loop, the envelope
handling and the `done` barrier, about ninety lines each. Any language that can read and write
lines works; there is nothing else to implement.

## Human / AI split

| Area | Olivier | Claude |
|---|---|---|
| Concept and scope | The idea, and what the tool is for | Nothing |
| Architecture and key decisions | The design, and every decision | Clarification, gaps and counter-arguments, on request |
| Implementation (`src/`, `templates/`) | The decisions it implements, and the guarantees the runtimes had to hold | All of the code, in Rust and four client languages |
| Tests | The approach: fuzz over seeds, determinism, replayability | The suite, and the cases that express it |
| Documentation | The scope, reviews and corrections | Most of the writing |
| Verification | Reviews of what's not boilerplate, and testing | Checking every claim by running it, and each test by breaking the code |

**What gated a merge.** Several of these pull requests ran past a thousand lines, and I did not
read them line by line. I read what carried the design and let the rest ride on two things. The
tests: 22 of them, each one checked to fail when the behaviour it guards breaks, which is a stronger
claim than "the tests pass". And use: I ran the tool and wrote a lab against it, which is where most
of my corrections came from.
