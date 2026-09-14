#!/usr/bin/env python3
"""Suite test fixture: reschedules a timer forever, so the run never runs out of events."""

import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "templates", "python"))
from cuelight import Node                                                    # noqa: E402

node = Node()


@node.on("init")
def _(src, body):
    node.set_timer(1, tick)


def tick():
    node.set_timer(1, tick)


node.run()
