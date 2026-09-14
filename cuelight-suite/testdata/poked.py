#!/usr/bin/env python3
"""Suite test fixture: reports every poke it receives, and does nothing else."""

import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "templates", "python"))
from cuelight import Node                                                    # noqa: E402

node = Node()


@node.on("poke")
def _(src, body):
    node.observe({"type": "poked", "id": body["id"]})


node.run()
