#!/usr/bin/env python3
"""Harness test fixture: exercises init, send, broadcast, timers and observable events."""

import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "templates", "python"))
from cuelight import Node                                                    # noqa: E402

node = Node()
state = {"rounds": 0, "acks": 0}


def hello():
    # The runtime provides no broadcast: whether "everyone" includes you is the caller's decision.
    for p in node.peers:
        if p != node.id:
            node.send(p, {"type": "hello"})


@node.on("init")
def _(src, body):
    hello()
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
        hello()
        node.set_timer(50)


node.run()
