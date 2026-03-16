"""Read-write lock for concurrent access to Chisel's storage."""

import threading
from contextlib import contextmanager


class RWLock:
    """A read-write lock allowing multiple concurrent readers or one exclusive writer.

    Uses write-preference: when a writer is waiting, new readers are blocked
    to prevent writer starvation. Existing readers finish normally.
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
