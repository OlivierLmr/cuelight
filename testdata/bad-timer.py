#!/usr/bin/env python3
"""Harness test fixture: arms one timer whose delay is $AFTER, evaluated as Python."""

import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "templates", "python"))
from cuelight import Node                                                    # noqa: E402

node = Node()


@node.on("init")
def _(src, body):
    node.set_timer(eval(os.environ["AFTER"]), lambda: node.observe({"type": "fired"}))


node.run()
