#!/usr/bin/env python3
"""Bounded PTY acceptance runner for fm (terminal-workspace track, Phase 12).

Launches the local fm binary on a real PTY inside temporary workspace,
config, and state directories — nothing touches the user's HOME, config,
or state. Every scenario is timeout-bounded and asserts child cleanup.

Entry point: python3 scripts/test-terminal-workspace.py
Env:
  FM_BIN   binary under test (default: <repo>/target/release/fm)
"""

import fcntl
import os
import pty
import select
import signal
import struct
import sys
import tempfile
import termios
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FM_BIN = os.environ.get("FM_BIN", os.path.join(REPO, "target", "release", "fm"))
TIMEOUT = 15.0
REAP_TIMEOUT = 5.0


def set_winsize(fd, rows, cols):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def make_dirs():
    """Isolated workspace + HOME/XDG surfaces."""
    workspace = tempfile.mkdtemp(prefix="fm-ws-")
    home = tempfile.mkdtemp(prefix="fm-home-")
    xdg = {
        "XDG_CONFIG_HOME": os.path.join(home, ".config"),
        "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
        "XDG_CACHE_HOME": os.path.join(home, ".cache"),
        "XDG_STATE_HOME": os.path.join(home, ".local", "state"),
    }
    for p in xdg.values():
        os.makedirs(p, exist_ok=True)
    return workspace, home, xdg


def spawn_fm(workspace, home, xdg, rows=24, cols=80, args=(), env_extra=None):
    pid, fd = pty.fork()
    if pid == 0:
        env = dict(
            os.environ,
            TERM="xterm-256color",
            HOME=home,
            **xdg,
            **(env_extra or {}),
        )
        os.execve(FM_BIN, [FM_BIN, workspace, *args], env)
        os._exit(127)
    set_winsize(fd, rows, cols)
    return pid, fd


def read_until(fd, needle, timeout=TIMEOUT, settle=0.6):
    """Drain output until needle (bytes) appears; returns the raw buffer.

    After the needle shows up, keep draining `settle` more seconds so
    asynchronous widgets that render in the same burst land in `buf`.
    """
    buf = b""
    deadline = time.time() + timeout
    settled_at = None
    while time.time() < deadline:
        r, _, _ = select.select([fd], [], [], 0.2)
        if r:
            try:
                buf += os.read(fd, 65536)
            except OSError:
                break
        if needle in buf:
            if settled_at is None:
                settled_at = time.time()
            elif time.time() - settled_at >= settle:
                break
    return buf


def reap(pid, timeout=REAP_TIMEOUT):
    """Bounded waitpid; SIGKILL fallback. Returns the exit status or raises."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done == pid:
            return status
        time.sleep(0.05)
    os.kill(pid, signal.SIGKILL)
    _, status = os.waitpid(pid, 0)
    raise TimeoutError(f"fm pid {pid} did not exit within {timeout}s (reaped {status})")


def assert_gone(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return
    raise AssertionError(f"fm pid {pid} still alive")


def scenario_smoke():
    """Launch on an 80x24 PTY, verify chrome + fixture file, quit cleanly."""
    workspace, home, xdg = make_dirs()
    with open(os.path.join(workspace, "hello.txt"), "w") as f:
        f.write("hello fm\n")
    pid, fd = spawn_fm(workspace, home, xdg)
    try:
        buf = read_until(fd, b"Commands")
        assert b"Commands" in buf, "status bar did not render [Commands] route"
        assert b"hello.txt" in buf, "fixture file not listed in the tree"
        os.write(fd, b"q")
        status = reap(pid)
        assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, (
            f"fm did not exit cleanly: {status}"
        )
        assert_gone(pid)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)


def reap_or_kill(pid):
    """Teardown guard: reap if zombie, kill if still running."""
    try:
        done, _ = os.waitpid(pid, os.WNOHANG)
        if done:
            return
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    except (ChildProcessError, ProcessLookupError):
        pass


SCENARIOS = [
    ("smoke: launch, chrome, clean quit, child reaped", scenario_smoke),
]


def main():
    if not os.path.isfile(FM_BIN):
        print(f"BLOCKED: fm binary missing at {FM_BIN} — run cargo build --release")
        return 2
    failures = 0
    for name, fn in SCENARIOS:
        start = time.time()
        try:
            fn()
            print(f"PASS {name} ({time.time() - start:.1f}s)")
        except Exception as exc:  # noqa: BLE001 — report, continue, exit nonzero
            failures += 1
            print(f"FAIL {name}: {exc}")
    print(f"{len(SCENARIOS) - failures}/{len(SCENARIOS)} scenarios passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
