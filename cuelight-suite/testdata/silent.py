#!/usr/bin/env python3
"""Suite test fixture: a node that answers nothing, so every liveness property fails."""

import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "templates", "python"))
from cuelight import Node                                                    # noqa: E402

Node().run()
