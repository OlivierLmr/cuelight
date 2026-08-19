# Templates

Student starter kits: ~40–70 lines each. They provide `on(type)` handler registration,
`send` / `broadcast` / `set_timer`, and emit `done` after every event.

They also own the **oracle switch**: the harness announces `"provided":["rb","omega"]` at init, and
the template's module shells forward to the harness for any layer listed. Student code is identical
either way — see `docs/design.md`, "Oracles".

Deliberately *not* like Maelstrom's templates: no threads, no blocking RPC, no wall-clock. A node
must be a pure event handler — same event sequence in, same actions out — or replay breaks.
