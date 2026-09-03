"""Minimal node runtime for cuelight.

A node is a *pure event handler*: it receives one event, emits its outgoing messages, and
announces `done`. Same event sequence in, same actions out.

Do not use wall-clock time, threads, or unseeded randomness. Replay depends on determinism,
and the harness will detect and report violations as your bug.
"""

import json
import sys


class Node:
    def __init__(self):
        self.id = None
        self.peers = []
        self.n = 0
        self.f = 0
        self.provided = []          # layers the harness is standing in for
        self._handlers = {}
        self._timer_cbs = {}
        self._out = []
        self._timer_id = 0

    # -- registration -------------------------------------------------------
    def on(self, msg_type):
        """Decorator: register a handler for one message type. `fn(src, body)`."""
        def register(fn):
            self._handlers[msg_type] = fn
            return fn
        return register

    # -- actions ------------------------------------------------------------
    def send(self, dest, body):
        self._out.append({"src": self.id, "dest": dest, "body": body})

    def set_timer(self, after, callback=None):
        """Fire after `after` units of LOGICAL time. Returns the timer id.

        With a callback, only that callback runs when it fires, which is what lets several
        layers hold timers at once without colliding.
        Timers cannot be cancelled; if you re-arm, guard the callback with a generation
        counter and ignore stale firings.
        """
        self._timer_id += 1
        if callback is not None:
            self._timer_cbs[self._timer_id] = callback
        self.send("harness", {"type": "set_timer",
                              "after": after,
                              "timer_id": self._timer_id})
        return self._timer_id

    def observe(self, body):
        """Report an observable event (deliver / enter_cs / leader / decide / ...)."""
        self.send("harness", body)

    def log(self, *args):
        print(*args, file=sys.stderr, flush=True)

    # -- main loop ----------------------------------------------------------
    def run(self):
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            msg = json.loads(line)
            body = msg["body"]

            if body["type"] == "init":
                self.id = body["node_id"]
                self.peers = body["node_ids"]
                self.n = body["n"]
                self.f = body["f"]
                self.provided = body.get("provided", [])

            if body["type"] == "timer":
                cb = self._timer_cbs.pop(body.get("timer_id"), None)
                if cb is not None:
                    cb()
                elif "timer" in self._handlers:
                    self._handlers["timer"](msg["src"], body)
            else:
                handler = self._handlers.get(body["type"])
                if handler:
                    handler(msg["src"], body)

            # `done` must be last: it is the barrier that lets logical time advance.
            self.send("harness", {"type": "done"})
            for m in self._out:
                sys.stdout.write(json.dumps(m) + "\n")
            sys.stdout.flush()
            self._out.clear()
