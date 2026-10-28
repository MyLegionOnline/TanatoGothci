"""Memory and time budget guards (design_vision_toolkit_plan.md Этап 1.3).

The workspace container's memory limit (1024 MB on hobby) is shared with the
dev server the agent may have already started in the same room — an OOM here
would take that server down too. So a script that can't fit its estimated
working set MUST refuse up front with a hint, never let numpy/Pillow hit the
cgroup limit and get killed.
"""
from __future__ import annotations

import time
from pathlib import Path


class BudgetError(RuntimeError):
    def __init__(self, message: str, hint: str):
        super().__init__(message)
        self.hint = hint


def _read_cgroup_v2_available_bytes() -> int | None:
    max_path = Path("/sys/fs/cgroup/memory.max")
    cur_path = Path("/sys/fs/cgroup/memory.current")
    if not max_path.exists() or not cur_path.exists():
        return None
    raw_max = max_path.read_text().strip()
    if raw_max == "max":
        return None
    limit = int(raw_max)
    used = int(cur_path.read_text().strip())
    return max(0, limit - used)


def _read_cgroup_v1_available_bytes() -> int | None:
    max_path = Path("/sys/fs/cgroup/memory/memory.limit_in_bytes")
    usage_path = Path("/sys/fs/cgroup/memory/memory.usage_in_bytes")
    if not max_path.exists() or not usage_path.exists():
        return None
    limit = int(max_path.read_text().strip())
    if limit > 2**62:  # "no limit" sentinel some kernels report
        return None
    used = int(usage_path.read_text().strip())
    return max(0, limit - used)


def _read_meminfo_available_bytes() -> int | None:
    path = Path("/proc/meminfo")
    if not path.exists():
        return None
    for line in path.read_text().splitlines():
        if line.startswith("MemAvailable:"):
            return int(line.split()[1]) * 1024
    return None


def available_bytes() -> int | None:
    """Best-effort available memory, or None if no known source is readable
    (e.g. running outside Linux/cgroups — tests on a dev machine)."""
    for reader in (
        _read_cgroup_v2_available_bytes,
        _read_cgroup_v1_available_bytes,
        _read_meminfo_available_bytes,
    ):
        try:
            value = reader()
        except (OSError, ValueError):
            value = None
        if value is not None:
            return value
    return None


def check_memory_budget(required_bytes: int, hint_example: str) -> None:
    avail = available_bytes()
    if avail is not None and avail < required_bytes:
        raise BudgetError(
            f"not enough memory: need ~{required_bytes // (1024 * 1024)} MiB, "
            f"~{avail // (1024 * 1024)} MiB available",
            hint=(
                "lower --max-pixels, close other work in this room, or retry "
                f"once the dev server has released memory. Example: {hint_example}"
            ),
        )


class TimeBudget:
    """Cooperative deadline — callers check `.expired` between recursive steps
    and stop subdividing, so a slow image degrades to a partial (`truncated:
    true`) result instead of running past the caller's patience."""

    def __init__(self, seconds: float = 20.0):
        self.deadline = time.monotonic() + seconds
        self.expired = False

    def check(self) -> bool:
        if not self.expired and time.monotonic() > self.deadline:
            self.expired = True
        return not self.expired
