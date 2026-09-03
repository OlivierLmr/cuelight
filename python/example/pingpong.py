#!/usr/bin/env python3
"""Ping-pong: the smallest program that exercises the whole interface.

n0 pings its first peer, which pongs back, and so on until a fixed number of rounds. It uses every
part of the template — handlers, `send`, `set_timer` with a callback, `observe` — and deliberately
implements no algorithm from any lab.

    cuelight run --seed 1 --bin python3 templates/python/example/pingpong.py
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from sdr import Node                                                    # noqa: E402

ROUNDS = 5
node = Node()
state = {"sent": 0}


def other():
    """The next node around the ring."""
    i = node.peers.index(node.id)
    return node.peers[(i + 1) % len(node.peers)]


@node.on("init")
def _(src, body):
    # Only n0 starts, so the ring carries exactly one token.
    if node.id == node.peers[0]:
        node.set_timer(10, ping)


def ping():
    if state["sent"] >= ROUNDS:
        return
    state["sent"] += 1
    node.send(other(), {"type": "ping", "n": state["sent"]})


@node.on("ping")
def _(src, body):
    node.observe({"type": "saw_ping", "n": body["n"], "from": src})
    node.send(src, {"type": "pong", "n": body["n"]})


@node.on("pong")
def _(src, body):
    node.observe({"type": "saw_pong", "n": body["n"]})
    node.set_timer(20, ping)


node.run()
