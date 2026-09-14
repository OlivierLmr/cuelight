# cuelight reference

Everything you look up rather than read: what crosses the wire, what cuelight can do to your nodes,
what a scenario contains, what a seed draws one from, and every option of both binaries.

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
| *anything else* | a **stimulus** from the space; its shape is yours to define |

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
per-link **FIFO** mode for algorithms that require ordered channels.

GST, the *Global Stabilisation Time*, is the instant after which delays become bounded. Before
it, the network may behave arbitrarily badly. It is what makes partial synchrony testable.

## Scenarios


A scenario, not a seed, is the replay unit. A seed would be consumed differently as your program
changes, so the same seed would stop meaning the same run. A scenario pins everything cuelight
controls, so it replays against any version of your code, can be shrunk by hand, and can be pasted
into a bug report.

```json
{
  "drawn": {"seed": 7, "space": "spaces/crashes.json", "fingerprint": "30a18569c5cd0c51"},
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

`drawn` is provenance, and only a drawn scenario has it: a seed names a run only relative to the
space it came from, and the fingerprint covers that space plus the version of the draw itself.
Edit a space and the fingerprint moves, which is the point. One written by hand came from nobody
and carries none of this.

Every other field has a default, so a scenario written by hand can be as small as
`{"nodes": 4, "stimuli": [...]}`. To author one, start from a drawn scenario and edit it:

```sh
cuelight scenario --seed 7 --space spaces/crashes.json > my-test.json
cuelight run --scenario my-test.json --bin ./my-node
```

## Spaces

A seed draws a scenario from **one** space: the world a run happens in, and a tree of the events
that befall it. Faults and stimuli are events of the same tree, because a crash and a request both
have an instant and a subject.

**Every field has the same shape**: a scalar pins it, a two-element array draws it from an
**inclusive** range. A pinned field consumes no randomness, so pinning one does not repoint the
draws after it.

```json
{ "f": [1, 3], "time_limit": 10000, "gst_frac": [0.10, 0.33],
  "link_delay_pre": [1, 400], "link_delay_post": [1, 25],
  "jitter_pct": 100, "fifo": true,
  "events": [
    { "nodes": { "distinct": 1 }, "at_frac": [0.0, 0.5],
      "stimulus": { "type": "ping", "id": "m<i>" },
      "then": [ { "delay": [1, 30], "crash": {} } ] },
    { "count": [0, 1], "at_frac": [0.0, 0.6], "partition": { "duration": [50, 600] } } ] }
```

That first event says something the format could not say before: *one process pings, and the same
one crashes shortly after*. The crash inherits its parent's process, which is why the language has
no variables.

`f` is the fault budget and the group size follows it, `n = 3f + 1`, so `[1, 3]` sweeps 4, 7 and 10.

**Who an event acts on**, and how many copies of it there are:

| | |
|---|---|
| `"nodes": "all"` | every process, once each |
| `"nodes": {"distinct": k}` | k processes not already taken on the path from the root, so under a parent it reads as *someone other than mine* |
| `"nodes": {"any": k}` | k draws with replacement |
| `"count": k` | k copies bound to no process, for an effect that acts on none |

**When**: a root places at `at_frac × time_limit`; a child at `parent + delay`, never negative, so
a child never precedes its parent and the tree admits no cycle. A delay of zero is the same logical
instant, which is how two events are made simultaneous; overlapping ranges on siblings leave their
order to the seed.

**What happens**: `stimulus` with a body the tool never reads, `crash: {}`, `pause` and `partition`
with a duration. A partition cuts a proper non-empty subset, drawn rather than declared.

Inside a body, `<i>` becomes the instance index and `{"$rand": [lo, hi]}` a drawn integer,
inclusive. Integer spans admit one symbol, `f`, and no arithmetic.

**A subtree is instantiated once per selected process**, so a tree multiplies: `distinct: [3, 7]`
over `distinct: [3, 7]` yields 9 to 49 events. Two levels read well; four do not.

**What a seed means.** The draw order is the depth-first walk, and within an event: subject, then
placement, then the body, then children. It is fixed by the schema, never by the order of keys in
the file. Inserting a sibling repoints every seed after it, which is what the fingerprint is for.

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
cuelight scenario [options]                 print a drawn scenario, to edit by hand
```

| Option | |
|---|---|
| `--bin <cmd...>` | command launching one node. **Must be last**, it swallows the rest of the line |
| `--scenario <path>` | replay a stored scenario instead of drawing one |
| `--seed <s>` | seed to draw from (default 1) |
| `--space <path>` | the space to draw from. Omitted, every field takes its default and nothing happens |
| `--watchdog <ms>` | wall-clock hang detector (5000) |
| `--out <dir>` | run directory (`store/latest`) |
| `--journal <path>` | the journal `viz` renders |

### The suite your binary gets

A binary built on `cuelight-suite` parses these for you, identically whatever it checks:

| Option | |
|---|---|
| `--bin <cmd...>` | command launching one node. **Must be last** |
| `--suite-dir <path>` | where this suite's `scenarios/` and `spaces/` live |
| `--out <dir>` | run directory (`store/<suite>`) |
| `--seeds <n>` | override every parametric scenario's seed count |
| `--seed <a>[..<b>]` | one seed, or an inclusive range, instead of every seed |
| `--only <name>` | run only what matches: a parametric scenario's name, or a written scenario's |
| `--watchdog <ms>` | wall-clock hang detector (5000) |
| `--list` | show what this suite declares, then exit |

`--seed` and `--only` are what make one failure cheap to look at: they run that case and nothing
else, and the failure told you which to type.
