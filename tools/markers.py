#!/usr/bin/env python3
"""Where bridge tools keep their read-position markers.

One place, because the location used to be hardcoded to a directory that only
exists on one Linux box: on macOS the write failed silently every call, the
marker never persisted, and bridge_wait re-announced every stale event forever
(the r21-r23 tooling defect list). The rule now: a per-user directory under
the platform's real temp root, created on first use, overridable with
BH_MARKER_DIR for tests and multi-match isolation.
"""
import getpass
import os
import tempfile
from pathlib import Path


def marker_dir() -> Path:
    """The directory markers live in, created if needed."""
    root = os.environ.get("BH_MARKER_DIR")
    if root:
        path = Path(root)
    else:
        try:
            user = getpass.getuser()
        except Exception:
            user = str(os.getuid()) if hasattr(os, "getuid") else "unknown"
        path = Path(tempfile.gettempdir()) / f"bridgehead-{user}"
    path.mkdir(parents=True, exist_ok=True)
    return path


def marker_path(prefix: str, key: str) -> str:
    """`<marker_dir>/<prefix>_<key-with-slashes-flattened>`."""
    return str(marker_dir() / (prefix + "_" + key.replace("/", "_")))
