#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Build the FM-1 hardware application and matching USB update package."""
from build_diag import main

if __name__ == "__main__":
    main(hardware_display=True)
