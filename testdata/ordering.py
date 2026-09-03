#!/usr/bin/env python3
"""Fixture: n0 blasts numbered messages at n1 in one event, so link jitter can reorder them.
n1 reports the order they arrive in."""
import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "templates", "python"))
from cuelight import Node                                                    # noqa: E402

node = Node()

@node.on("init")
def _(src, body):
    if node.id == "n0":
        for i in range(12):
            node.send("n1", {"type": "seq", "i": i})

@node.on("seq")
def _(src, body):
    node.observe({"type": "deliver", "mid": body["i"]})

node.run()
