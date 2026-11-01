#!/usr/bin/env python3
"""Compatibility entrypoint: native Files review now uses the persistent workbench.

Keep the original smoke command usable; the actual production-main PTY flow lives
in qa-pi-workbench.py. This is an owned offline fixture, never an owner pane.
"""
import runpy
from pathlib import Path
runpy.run_path(str(Path(__file__).with_name("qa-pi-workbench.py")), run_name="__main__")
