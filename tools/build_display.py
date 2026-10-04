#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Build the emulator-only foundation firmware with live LCD output."""
from build_foundation import main

if __name__ == "__main__":
    main(display=True)
