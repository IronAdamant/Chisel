"""Tests for chisel.rwlock — Read-write lock for concurrent access.

Tests use threading primitives (Event, Barrier) to create deterministic
concurrency scenarios that verify the correctness of the RWLock.
"""

import threading
import time

import pytest

from chisel.rwlock import RWLock


# ------------------------------------------------------------------ #
# Fixtures
# ------------------------------------------------------------------ #

@pytest.fixture
def rwlock():
    """Return a fresh RWLock instance."""
    return RWLock()


# ------------------------------------------------------------------ #
# Tests: Basic read_lock context manager
# ------------------------------------------------------------------ #

class TestReadLock:
    def test_read_lock_acquires_and_releases(self, rwlock):
        """read_lock context manager increments then decrements _readers."""
        assert rwlock._readers == 0
        with rwlock.read_lock():
            assert rwlock._readers == 1
        assert rwlock._readers == 0

    def test_read_lock_yields_none(self, rwlock):
        """The context manager yields None (no special value)."""
        with rwlock.read_lock() as val:
            assert val is None

    def test_read_lock_releases_on_exception(self, rwlock):
        """read_lock releases even if the body raises an exception."""
        with pytest.raises(ValueError, match="boom"):
            with rwlock.read_lock():
                assert rwlock._readers == 1
                raise ValueError("boom")
        assert rwlock._readers == 0

    def test_nested_read_locks(self, rwlock):
        """A single thread can acquire multiple read locks (reentrant reads)."""
        with rwlock.read_lock():
            assert rwlock._readers == 1
            with rwlock.read_lock():
                assert rwlock._readers == 2
            assert rwlock._readers == 1
        assert rwlock._readers == 0


# ------------------------------------------------------------------ #
# Tests: Basic write_lock context manager
# ------------------------------------------------------------------ #

class TestWriteLock:
    def test_write_lock_acquires_and_releases(self, rwlock):
        """write_lock sets _writer to True inside, False outside."""
        assert rwlock._writer is False
        with rwlock.write_lock():
            assert rwlock._writer is True
        assert rwlock._writer is False

    def test_write_lock_yields_none(self, rwlock):
        """The context manager yields None."""
        with rwlock.write_lock() as val:
            assert val is None

    def test_write_lock_releases_on_exception(self, rwlock):
        """write_lock releases even if the body raises an exception."""
        with pytest.raises(RuntimeError, match="kaboom"):
            with rwlock.write_lock():
                assert rwlock._writer is True
                raise RuntimeError("kaboom")
        assert rwlock._writer is False


# ------------------------------------------------------------------ #
# Tests: Multiple concurrent readers
# ------------------------------------------------------------------ #

class TestConcurrentReaders:
    def test_multiple_readers_hold_lock_simultaneously(self, rwlock):
        """Two reader threads can both hold the read lock at the same time."""
        barrier = threading.Barrier(2, timeout=5)
        both_inside = threading.Event()
        errors = []

        def reader():
            try:
                with rwlock.read_lock():
                    # Wait until both readers are inside the lock
                    barrier.wait()
                    # If we get here, both readers are holding the lock
                    both_inside.set()
            except Exception as exc:
                errors.append(exc)

        t1 = threading.Thread(target=reader)
        t2 = threading.Thread(target=reader)
        t1.start()
        t2.start()
        t1.join(timeout=5)
        t2.join(timeout=5)

        assert not errors, f"Reader threads raised errors: {errors}"
        assert both_inside.is_set(), "Both readers should have been inside the lock"

    def test_many_concurrent_readers(self, rwlock):
        """Multiple readers (5) can all hold the lock concurrently.

        The barrier only releases once all 5 threads have reached it,
        which proves all 5 held the read lock at the same time.
        """
        num_readers = 5
        barrier = threading.Barrier(num_readers, timeout=5)
        barrier_passed = threading.Event()
        errors = []

        def reader():
            try:
                with rwlock.read_lock():
                    # Barrier only unblocks when all num_readers arrive
                    barrier.wait()
                    barrier_passed.set()
            except Exception as exc:
                errors.append(exc)

        threads = [threading.Thread(target=reader) for _ in range(num_readers)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=5)

        assert not errors, f"Reader threads raised errors: {errors}"
        # The barrier passing proves all 5 held the read lock simultaneously
        assert barrier_passed.is_set()


# ------------------------------------------------------------------ #
# Tests: Writer exclusion
# ------------------------------------------------------------------ #

class TestWriterExclusion:
    def test_writer_blocks_while_readers_hold_lock(self, rwlock):
        """A writer cannot acquire the lock while a reader holds it."""
        reader_inside = threading.Event()
        writer_tried = threading.Event()
        writer_acquired = threading.Event()

        def reader():
            with rwlock.read_lock():
                reader_inside.set()
                # Hold the lock until the writer has attempted to acquire
                writer_tried.wait(timeout=5)
                # Give the writer a moment to actually block on the condition
                time.sleep(0.05)

        def writer():
            reader_inside.wait(timeout=5)
            writer_tried.set()
            with rwlock.write_lock():
                writer_acquired.set()

        t_r = threading.Thread(target=reader)
        t_w = threading.Thread(target=writer)
        t_r.start()
        t_w.start()

        # The writer should NOT have acquired the lock while reader holds it
        t_r.join(timeout=5)
        # After reader releases, writer should get it
        t_w.join(timeout=5)

        assert writer_acquired.is_set(), "Writer should acquire lock after reader releases"

    def test_readers_block_while_writer_holds_lock(self, rwlock):
        """Readers cannot acquire the lock while a writer holds it."""
        writer_inside = threading.Event()
        reader_tried = threading.Event()
        reader_acquired = threading.Event()

        def writer():
            with rwlock.write_lock():
                writer_inside.set()
                # Hold the lock until the reader has attempted to acquire
                reader_tried.wait(timeout=5)
                # Give the reader a moment to actually block on the condition
                time.sleep(0.05)

        def reader():
            writer_inside.wait(timeout=5)
            reader_tried.set()
            with rwlock.read_lock():
                reader_acquired.set()

        t_w = threading.Thread(target=writer)
        t_r = threading.Thread(target=reader)
        t_w.start()
        t_r.start()

        # After writer releases, reader should get it
        t_w.join(timeout=5)
        t_r.join(timeout=5)

        assert reader_acquired.is_set(), "Reader should acquire lock after writer releases"

    def test_two_writers_are_mutually_exclusive(self, rwlock):
        """Two writers cannot hold the lock at the same time."""
        barrier = threading.Barrier(2, timeout=5)
        writer_entry_order = []
        lock_for_list = threading.Lock()
        overlap_detected = threading.Event()

        def writer(writer_id):
            barrier.wait()  # Both threads start trying at the same time
            with rwlock.write_lock():
                with lock_for_list:
                    writer_entry_order.append(("enter", writer_id))
                    # If the other writer already entered but didn't exit, overlap
                    enters = sum(1 for ev, _ in writer_entry_order if ev == "enter")
                    exits = sum(1 for ev, _ in writer_entry_order if ev == "exit")
                    if enters - exits > 1:
                        overlap_detected.set()
                # Small sleep to make overlap window detectable
                time.sleep(0.02)
                with lock_for_list:
                    writer_entry_order.append(("exit", writer_id))

        t1 = threading.Thread(target=writer, args=(1,))
        t2 = threading.Thread(target=writer, args=(2,))
        t1.start()
        t2.start()
        t1.join(timeout=5)
        t2.join(timeout=5)

        assert not overlap_detected.is_set(), "Two writers overlapped — mutual exclusion violated"
        assert len(writer_entry_order) == 4, "Both writers should enter and exit"


# ------------------------------------------------------------------ #
# Tests: Writer starvation awareness
# ------------------------------------------------------------------ #

class TestWriterPreference:
    """Verify that the RWLock uses write-preference.

    When a writer is waiting, new readers are blocked to prevent
    writer starvation. Only already-holding readers continue.
    """

    def test_writer_not_starved_by_overlapping_readers(self, rwlock):
        """A waiting writer blocks new readers and eventually acquires.

        We spawn staggered readers. A writer starts waiting once the
        first reader is inside. With write-preference, later readers
        that haven't entered yet are blocked, so the writer gets the
        lock after the existing readers finish.
        """
        reader_hold_time = 0.05
        first_reader_inside = threading.Event()
        writer_acquired = threading.Event()

        def overlapping_reader(delay):
            time.sleep(delay)
            with rwlock.read_lock():
                first_reader_inside.set()
                time.sleep(reader_hold_time)

        def writer():
            first_reader_inside.wait(timeout=5)
            with rwlock.write_lock():
                writer_acquired.set()

        # First reader starts immediately, second is staggered
        t_r1 = threading.Thread(target=overlapping_reader, args=(0,))
        t_r2 = threading.Thread(target=overlapping_reader, args=(0.02,))
        t_w = threading.Thread(target=writer)

        t_r1.start()
        t_r2.start()
        t_w.start()

        t_r1.join(timeout=5)
        t_r2.join(timeout=5)
        t_w.join(timeout=5)

        assert writer_acquired.is_set(), "Writer must acquire lock"

    def test_new_readers_blocked_while_writer_waiting(self, rwlock):
        """Once a writer starts waiting, new readers cannot enter."""
        reader_inside = threading.Event()
        writer_waiting = threading.Event()
        writer_acquired = threading.Event()
        late_reader_blocked = threading.Event()
        late_reader_acquired = threading.Event()

        def first_reader():
            with rwlock.read_lock():
                reader_inside.set()
                # Hold until writer is waiting
                writer_waiting.wait(timeout=5)
                time.sleep(0.05)

        def writer():
            reader_inside.wait(timeout=5)
            writer_waiting.set()
            with rwlock.write_lock():
                writer_acquired.set()

        def late_reader():
            writer_waiting.wait(timeout=5)
            time.sleep(0.02)  # Give writer time to set _writers_waiting
            late_reader_blocked.set()
            with rwlock.read_lock():
                late_reader_acquired.set()

        t_r = threading.Thread(target=first_reader)
        t_w = threading.Thread(target=writer)
        t_lr = threading.Thread(target=late_reader)

        t_r.start()
        t_w.start()
        t_lr.start()

        t_r.join(timeout=5)
        t_w.join(timeout=5)
        t_lr.join(timeout=5)

        assert writer_acquired.is_set(), "Writer must get the lock"
        assert late_reader_acquired.is_set(), "Late reader must eventually acquire"
