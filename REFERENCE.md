# cuelight reference

Everything you look up rather than read: what crosses the wire, what cuelight can do to your nodes,
what a scenario contains, how a workload template expands, and every option of both binaries.

Start at the [README](README.md); the journal format has its own file, [JOURNAL.md](JOURNAL.md).

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

Plugging in your own checker means nothing more than reading that file after the run. The simulator
has no sweep of its own, because pruning the runs that passed would mean knowing what passing is.
`cuelight-suite` sweeps precisely because you give it that knowledge, as a function.

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
| `--journal <path>` | the journal `viz` renders |

### The suite your binary gets

A binary built on `cuelight-suite` parses these for you, identically whatever it checks:

| Option | |
|---|---|
| `--bin <cmd...>` | command launching one node. **Must be last** |
| `--suite-dir <path>` | where this suite's `scenarios/` and `stimuli/` live |
| `--out <dir>` | run directory (`store/<suite>`) |
| `--seeds <n>` | override every campaign's seed count |
| `--seed <a>[..<b>]` | one seed, or an inclusive range, instead of a whole campaign |
| `--only <name>` | run only what matches: a campaign label or a scenario name |
| `--watchdog <ms>` | wall-clock hang detector (5000) |
| `--list` | show what this suite declares, then exit |

`--seed` and `--only` are what make one failure cheap to look at: they run that case and nothing
else, and the failure told you which to type.
