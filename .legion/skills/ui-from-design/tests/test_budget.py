"""Этап 1.5's memory-budget requirement: refuse with a clear hint instead of
letting numpy/Pillow run into the container's cgroup limit and OOM (which
would also take down any dev server sharing the same 1024 MB room). The
production check reads /sys/fs/cgroup and /proc/meminfo, neither of which
exist on a Windows dev machine — these tests exercise the pure decision
logic (`check_memory_budget`) against a mocked `available_bytes`, which is
portable and exercises the exact same code path CI would hit on Linux."""
import time

import pytest

from vlib import budget


def test_check_memory_budget_raises_when_insufficient(monkeypatch):
    monkeypatch.setattr(budget, "available_bytes", lambda: 100 * 1024 * 1024)  # 100 MiB free
    with pytest.raises(budget.BudgetError) as exc_info:
        budget.check_memory_budget(500 * 1024 * 1024, hint_example="vision.py probe --image x.png")
    assert "hint" in dir(exc_info.value)
    assert exc_info.value.hint


def test_check_memory_budget_passes_when_sufficient(monkeypatch):
    monkeypatch.setattr(budget, "available_bytes", lambda: 900 * 1024 * 1024)
    budget.check_memory_budget(100 * 1024 * 1024, hint_example="vision.py probe --image x.png")  # must not raise


def test_check_memory_budget_never_raises_when_source_unknown(monkeypatch):
    # No cgroup/meminfo readable (e.g. non-Linux) -> available_bytes() is
    # None -> fail open, don't block every call just because we can't tell.
    monkeypatch.setattr(budget, "available_bytes", lambda: None)
    budget.check_memory_budget(10**12, hint_example="vision.py probe --image x.png")


def test_time_budget_expires_after_deadline():
    tb = budget.TimeBudget(seconds=0.01)
    assert tb.check() is True
    time.sleep(0.05)
    assert tb.check() is False
    assert tb.expired is True


def test_time_budget_does_not_expire_early():
    tb = budget.TimeBudget(seconds=5.0)
    assert tb.check() is True
    assert tb.expired is False
