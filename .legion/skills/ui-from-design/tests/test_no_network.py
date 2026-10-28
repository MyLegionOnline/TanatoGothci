"""Этап 1.5: no subcommand goes to the network — CV-only, deterministic, and
free to call (design_vision_toolkit_plan.md Р5/Р6). Enforced statically:
every script must be free of network-capable imports, not just "didn't
happen to call one in this test run"."""
import ast
from pathlib import Path

FORBIDDEN_MODULES = {
    "socket",
    "requests",
    "urllib",
    "urllib.request",
    "http.client",
    "httpx",
    "aiohttp",
    "ftplib",
    "smtplib",
    "telnetlib",
}

SCRIPTS_DIR = Path(__file__).resolve().parents[1] / "scripts"


def _python_files():
    return sorted(SCRIPTS_DIR.rglob("*.py"))


def _imported_modules(tree: ast.Module) -> set[str]:
    modules = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                modules.add(alias.name)
        elif isinstance(node, ast.ImportFrom) and node.module:
            modules.add(node.module)
    return modules


def test_no_python_file_imports_networking_modules():
    files = _python_files()
    assert files, "expected to find vision.py and vlib/*.py"
    offenders = {}
    for path in files:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        found = _imported_modules(tree) & FORBIDDEN_MODULES
        if found:
            offenders[str(path.relative_to(SCRIPTS_DIR))] = found
    assert not offenders, f"network-capable imports found: {offenders}"
