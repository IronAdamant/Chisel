"""Read-write lock for concurrent access to Chisel's storage."""

import threading
from contextlib import contextmanager


class RWLock:
    """A read-write lock allowing multiple concurrent readers or one exclusive writer.

    Uses write-preference: when a writer is waiting, new readers are blocked
    to prevent writer starvation. Existing readers finish normally.

    Note: Neither lock is reentrant. ``write_lock`` will deadlock if the
    same thread acquires it twice. ``read_lock`` can also deadlock if
    re-entered while a writer is waiting (the reader blocks on the writer
    wait while still holding a read count, preventing the writer from
    proceeding).
    """

    def __init__(self):
        self._lock = threading.Lock()
        self._cond = threading.Condition(self._lock)
        self._readers = 0
        self._writer = False
        self._writers_waiting = 0

    @contextmanager
    def read_lock(self):
        with self._cond:
            while self._writer or self._writers_waiting:
                self._cond.wait()
            self._readers += 1
        try:
            yield
        finally:
            with self._cond:
                self._readers -= 1
                if self._readers == 0:
                    self._cond.notify_all()

    @contextmanager
    def write_lock(self):
        with self._cond:
            self._writers_waiting += 1
            try:
                while self._writer or self._readers > 0:
                    self._cond.wait()
            finally:
                self._writers_waiting -= 1
            self._writer = True
        try:
            yield
        finally:
            with self._cond:
                self._writer = False
                self._cond.notify_all()
