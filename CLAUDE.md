# Working in this repository

cuelight is a deterministic discrete-event simulator for distributed systems. It executes one
scenario and records what happened. **It never judges a run** — no notion of success, no list of
message types, no idea what any protocol word means. Everything that decides what a run is worth
lives outside it.

## The documents that matter

| File | What it is, and when to read it |
|---|---|
| [JOURNAL.md](JOURNAL.md) | The journal format, which is the public interface. Read it before changing anything a checker or a renderer parses; changing it is a versioned event |
| [REFERENCE.md](REFERENCE.md) | Every command, flag and scenario field. Keep it in step with the CLI in the same commit |
| [cuesheet.cuestyle](cuesheet.cuestyle) | What the words in a `--format cuesheet` document are, and how they look |

## Two invariants worth stating

**The harness holds no vocabulary.** Anything a node sends to the harness that is not `set_timer` or
`done` is an `observe`, recorded verbatim. It once kept a whitelist of six names taken from one
project's exercises, which is exactly the coupling that made it unusable for anything else. Do not
reintroduce one — not in the simulator, not in the emitters.

**cuelight never depends on [cuesheet](https://github.com/OlivierLmr/cuesheet).** The emitter writes
a text document and stops; the renderer is a separate tool a reader installs if they want it. The
course's own words (`enter_cs`, `do_broadcast`, …) belong in a style sheet, never in `src/`.

## Version control

Recorded from practice, so nobody re-derives it from `git log`:

1. **A branch per change**, then a pull request. Never commit a change straight to `main`.
2. **Squash-merge** the PR, with the subject `<the PR title> (#NN)`. GitHub closes the issue the
   body names.
3. **Bump the version in its own commit** afterwards, subject `vX.Y.Z: <headline of what shipped>`,
   touching `Cargo.toml`, `cuelight-suite/Cargo.toml` and `Cargo.lock` and nothing else.
4. **An annotated tag** `vX.Y.Z` carrying the release notes, then push `main` and the tag.
5. Merged branches are **kept**, not deleted.

Commits carry `Co-authored-by: Claude Opus 5 (1M context) <noreply@anthropic.com>`.

**A change to the journal moves the minor version**, because a checker or a renderer reads it and a
silent change breaks them elsewhere. v0.6.0 was `mid`.

**Ask before merging a PR, tagging, or pushing to `main`** — they are outward-facing and awkward to
undo. Authorisation given for one release does not carry to the next.

## Testing

`cargo test --all`. The determinism test drives the real binary and takes **two minutes on its own**,
so allow a generous timeout and do not read a partial log as a pass — check the exit status, since a
`FAILED` line sits above the `test result: ok` lines of the suites that did pass.
