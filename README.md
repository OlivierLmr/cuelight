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

## Two crates

| Crate | What it is |
|---|---|
| `cuelight` | the simulator: a library, and a command line over it |
| `cuelight-suite` | a runner over that library: many scenarios, and your verdict on each |

```toml
[dependencies]
cuelight-suite = { git = "https://github.com/OlivierLmr/cuelight", tag = "v0.1.0" }
```

The simulator gives you one run and its journal. Judging that run, sweeping seeds and keeping only
the failures is a different job, and `cuelight-suite` does it without ever learning what your
properties are: you hand it a `Suite` and a function, and it calls the function.

```rust
use cuelight_suite::{Campaign, Events, Kind, Report, Scenario, Suite};
use std::process::ExitCode;

static CAMPAIGNS: &[Campaign] = &[Campaign {
    label: "steady", stimuli: "stimuli/steady.json", fifo: true, faults: true, seeds: 200,
}];

fn check(ev: &Events, _sc: &Scenario, r: &mut Report) {
    r.add("said-something", Kind::Safety, !ev.observes.is_empty(), "...".into());
}

fn main() -> ExitCode {
    cuelight_suite::run(
        Suite { name: "mine", campaigns: CAMPAIGNS, directed: &[], check },
        env!("CARGO_MANIFEST_DIR"),
    )
}
```

That binary replays one case before judging anything, and stops if your node does not reproduce it.
Then it runs every campaign, deletes the seeds that passed, and prints under each failure the
command that runs it again, the journal, and a sequence diagram.

## Your side of the contract

A node is a **pure event handler**: same event sequence in, same actions out. Three rules follow,
and cuelight enforces all three.

1. **Emit `done` after every event, last.** It is the barrier that lets logical time advance.
   Without it you are declared hung.
2. **Never exit on your own.** An unscheduled exit fails the run. cuelight knows which crashes it
   injected, and a program that dies on unexpected input must not pass as surviving them.
3. **No wall-clock, no threads, no unseeded randomness.** Replay depends on it.

## A complete example

`templates/*/example/` holds the same ping-pong node in Python, Go, Java and C++: n0 pings the next
node around the ring, which pongs back, five rounds. Forty lines, touching handler registration,
`send`, `set_timer` with a callback and `observe`.

```sh
cuelight run --seed 1 --no-faults --bin python3 templates/python/example/pingpong.py
```

The five rounds land in `store/latest/journal.jsonl`. The first three, times included:

```
358 n1 saw_ping 1     985 n1 saw_ping 2    1469 n1 saw_ping 3
696 n0 saw_pong 1    1200 n0 saw_pong 2    1470 n0 saw_pong 3
```

All four languages produce those observations at those times. Three of them line for line: Go's
JSON encoder sorts the keys of an object, so its bodies carry the same fields in another order. If
yours matches on times and contents, your plumbing is right.

## Where to look

| | |
|---|---|
| [REFERENCE.md](REFERENCE.md) | the wire protocol, the faults, scenarios, workloads, every option |
| [JOURNAL.md](JOURNAL.md) | the journal format, which is what your checker parses |
| [templates/](templates/) | the node runtime in four languages, ninety lines each, and the example above |

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
tests: 28 of them, each one checked to fail when the behaviour it guards breaks, which is a stronger
claim than "the tests pass". And use: I used it in the context of my own course, which is where most
of my corrections came from.
