#!/usr/bin/env python3
"""Fixture: n0 sends one numbered message per step at n1, which a pause or a partition holds.
n1 reports the order they arrive in once the hold lifts."""
import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "templates", "python"))
from cuelight import Node                                                    # noqa: E402

node = Node()

@node.on("init")
def _(src, body):
    if node.id == "n0":
        for i in range(6):
            node.set_timer(100 * (i + 1), lambda i=i: node.send("n1", {"type": "seq", "i": i}))

@node.on("seq")
def _(src, body):
    node.observe({"type": "deliver", "mid": body["i"]})

node.run()
