#!/usr/bin/env python3
"""Harness test fixture: exercises init, send, broadcast, timers and observable events."""

import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "templates", "python"))
from sdr import Node                                                    # noqa: E402

node = Node()
state = {"rounds": 0, "acks": 0}


@node.on("init")
def _(src, body):
    node.broadcast({"type": "hello"})
    node.set_timer(50)


@node.on("hello")
def _(src, body):
    node.send(src, {"type": "ack"})


@node.on("ack")
def _(src, body):
    state["acks"] += 1
    node.observe({"type": "deliver", "mid": f"ack-from-{src}"})


@node.on("timer")
def _(src, body):
    if state["rounds"] < 3:
        state["rounds"] += 1
        node.broadcast({"type": "hello"})
        node.set_timer(50)


node.run()
