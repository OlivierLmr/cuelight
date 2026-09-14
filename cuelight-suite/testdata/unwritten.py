#!/usr/bin/env python3
"""Suite test fixture: a skeleton nobody has filled in yet, which is how every student starts."""

import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "templates", "python"))
from cuelight import Node                                                    # noqa: E402

node = Node()


@node.on("poke")
def _(src, body):
    raise NotImplementedError("à vous de jouer")


node.run()
