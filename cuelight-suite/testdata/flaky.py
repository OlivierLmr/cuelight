#!/usr/bin/env python3
"""Suite test fixture: reads the wall clock, so two runs of one scenario differ."""

import sys, os, time
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "templates", "python"))
from cuelight import Node                                                    # noqa: E402

node = Node()


@node.on("poke")
def _(src, body):
    node.observe({"type": "poked", "id": str(time.time_ns())})


node.run()
