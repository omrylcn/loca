"""Small cross-platform advisory file locks for Loca's local runtimes."""

from __future__ import annotations

import os

try:
    import fcntl
except ImportError:  # Windows
    fcntl = None
    import msvcrt


def lock_file(fd: int, *, blocking: bool = True) -> None:
    """Exclusively lock an open descriptor on POSIX or Windows."""
    if fcntl is not None:
        flags = fcntl.LOCK_EX
        if not blocking:
            flags |= fcntl.LOCK_NB
        fcntl.flock(fd, flags)
        return

    # msvcrt.locking locks bytes from the current offset. Ensure the lock file
    # owns one byte, then always lock byte zero. Closing the descriptor releases
    # the lock, matching flock's lifetime in the callers.
    if os.fstat(fd).st_size == 0:
        os.write(fd, b"L")
    os.lseek(fd, 0, os.SEEK_SET)
    mode = msvcrt.LK_LOCK if blocking else msvcrt.LK_NBLCK
    msvcrt.locking(fd, mode, 1)
