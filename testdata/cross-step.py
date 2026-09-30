#!/usr/bin/env python3
"""Fixture: n0 sends a burst to n1, then answers a ping that lands mid-burst, on the same link.
n1 reports the order they arrive in. The answer was made last, so it must arrive last."""
import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "templates", "python"))
from cuelight import Node                                                    # noqa: E402

node = Node()

@node.on("init")
def _(src, body):
    if node.id == "n0":
        for i in range(6):
            node.send("n1", {"type": "seq", "i": i})
    if node.id == "n1":
        node.send("n0", {"type": "ping"})

@node.on("ping")
def _(src, body):
    node.send(src, {"type": "seq", "i": 99})

@node.on("seq")
def _(src, body):
    node.observe({"type": "deliver", "mid": body["i"]})

node.run()
