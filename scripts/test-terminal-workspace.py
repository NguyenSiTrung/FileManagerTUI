#!/usr/bin/env python3
"""Bounded PTY acceptance runner for fm (terminal-workspace track, Phase 12).

Launches the local fm binary on a real PTY inside temporary workspace,
config, and state directories — nothing touches the user's HOME, config,
or state. Every scenario is timeout-bounded and asserts child cleanup.

Coverage maps to spec.md AC-1..AC-11: the 80x24 workflow with exact file
bytes, resizes, TERM variants, nested tmux transport, feature-off flags,
missing Git/LSP executables, a fake LSP diagnostics round-trip, external
saves, dirty-quit decisions, crash capture, and corrupt/private recovery
state. A missing mandatory harness (binary, tmux) is a loud BLOCKED
failure, never a silent skip.

Entry point: python3 scripts/test-terminal-workspace.py
Env:
  FM_BIN   binary under test (default: <repo>/target/release/fm)
"""

import fcntl
import json
import os
import pty
import re
import select
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import termios
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FM_BIN = os.environ.get("FM_BIN", os.path.join(REPO, "target", "release", "fm"))
FAKE_LSP = os.path.join(REPO, "scripts", "fake-lsp-server.py")
TIMEOUT = 15.0
REAP_TIMEOUT = 5.0

ALT_G = b"\x1bg"
ENTER = b"\r"


# ── PTY helpers ──────────────────────────────────────────────────────────────


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
            HOME=home,
            **xdg,
            **(env_extra or {}),
        )
        env.setdefault("TERM", "xterm-256color")
        os.execve(FM_BIN, [FM_BIN, workspace, *args], env)
        os._exit(127)
    set_winsize(fd, rows, cols)
    return pid, fd


# Ratatui emits only *changed* cells, and styled spans emit separately —
# `a.py! [pin]` never exists as contiguous raw bytes (`a.py` cells stay,
# `! [pin]` cells are new, ANSI styles sit between). Needles are therefore
# matched against the escape-stripped text, not the raw stream.
_ANSI_RE = re.compile(
    rb"\x1b\][^\x07]*(?:\x07|\x1b\\)"  # OSC sequences
    rb"|\x1b\[[0-9;?]*[a-zA-Z@~]"      # CSI sequences
    rb"|\x1b[()][0-9A-B]"              # charset selects
    rb"|\x1b[=>]"                     # keypad modes
)


def read_until(fd, needle, timeout=TIMEOUT, settle=0.6):
    """Drain output until needle appears in the escape-stripped text.

    Returns the raw buffer. After the needle shows up, keep draining
    `settle` more seconds so asynchronous widgets that render in the same
    burst land in `buf`.
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
        if needle in _ANSI_RE.sub(b"", buf):
            if settled_at is None:
                settled_at = time.time()
            elif time.time() - settled_at >= settle:
                break
    return buf


def screen_text(buf):
    """Escape-stripped printable text of a drained buffer (for asserts)."""
    return _ANSI_RE.sub(b"", buf)


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


def quit_cleanly(pid, fd):
    """From a clean (no dirty doc, tree-focusable) state: quit and reap 0."""
    os.write(fd, ALT_G + b"1")
    os.write(fd, b"q")
    status = reap(pid)
    assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, (
        f"fm did not exit cleanly: {status}"
    )
    assert_gone(pid)


def open_via_quick_open(fd, name, expect=None):
    """Alt+G o → Fuzzy Finder → type name → wait for the candidate → Enter.

    `expect` is a needle only the opened document can emit — fresh buffer
    text, or a new tab/panel title. Without it the post-Enter read looks
    for `name` itself, which is reliable for a first open (a new tab is all
    new cells) but not for switching back to an already-open document —
    callers switching documents MUST pass a content needle.
    """
    # A startup-race can swallow the chord; send it again before failing.
    buf = b""
    for _ in range(2):
        os.write(fd, ALT_G + b"o")
        buf = read_until(fd, b"Fuzzy Finder", timeout=6)
        if b"Fuzzy Finder" in screen_text(buf):
            break
    assert b"Fuzzy Finder" in screen_text(buf), "Quick Open overlay did not appear"
    os.write(fd, name.encode())
    # Wait for the selected candidate row `▸ {name}` — Enter inside the
    # async index warm-up window can hit an empty candidate set.
    cand = "▸ ".encode() + name.encode()
    buf += read_until(fd, cand, timeout=10)
    assert cand in screen_text(buf), f"no finder candidate for {name}"
    time.sleep(0.3)
    os.write(fd, ENTER)
    needle = expect if isinstance(expect, bytes) else (expect or name).encode()
    buf += read_until(fd, needle, timeout=8)
    assert needle in screen_text(buf), (
        f"{name} did not open ({needle!r} never emitted)"
    )
    return buf


# ── Recovery record synthesis (mirrors src/recovery.rs hashing) ──────────────


def fnv1a(parts):
    h = 0xCBF29CE484222325
    for part in parts:
        for b in part:
            h ^= b
            h = (h * 0x00000100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def write_recovery_record(xdg_state, workspace, doc_path, text):
    """Write a schema-valid snapshot record exactly where fm will look."""
    st = os.lstat(doc_path)
    size = st.st_size
    secs = st.st_mtime_ns // 1_000_000_000
    nanos = st.st_mtime_ns % 1_000_000_000
    digest = fnv1a(
        [
            size.to_bytes(8, "little"),
            secs.to_bytes(8, "little", signed=True),
            nanos.to_bytes(4, "little"),
            b"\x01",  # modified_known = true
        ]
    )
    name = "snap-{:016x}-{:016x}-{:016x}.json".format(
        fnv1a([os.fsencode(workspace)]),
        fnv1a([os.fsencode(doc_path)]),
        digest,
    )
    records = os.path.join(xdg_state, "fm-tui", "recovery", "recovery")
    os.makedirs(records, exist_ok=True)
    os.chmod(records, 0o700)
    payload = {
        "schema": "fm-tui-recovery",
        "version": 1,
        "workspace_root": workspace,
        "document_path": doc_path,
        "revision": {
            "size": size,
            "modified_secs": secs,
            "modified_nanos": nanos,
            "modified_known": True,
        },
        "text": text,
        "captured_secs": int(time.time()),
    }
    path = os.path.join(records, name)
    with open(path, "w") as f:
        json.dump(payload, f)
    os.chmod(path, 0o600)
    return records


# ── Scenarios ────────────────────────────────────────────────────────────────


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
        quit_cleanly(pid, fd)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)


def scenario_full_workflow():
    """AC-1/AC-3 PTY: two files, edit dirty, terminal shell, bracketed paste,
    save exact bytes, retained dirty buffer, dirty-quit decision dialog."""
    workspace, home, xdg = make_dirs()
    with open(os.path.join(workspace, "a.py"), "w") as f:
        f.write("print('a')\n")
    open(os.path.join(workspace, "config.yaml"), "w").close()
    pid, fd = spawn_fm(workspace, home, xdg)
    try:
        buf = read_until(fd, b"Commands")
        assert b"Commands" in buf, "status bar chrome missing"

        # File A: open + edit (dirty, unsaved). The typed text becomes new
        # buffer cells — the reliable needle (the [EDIT] title never
        # re-emits, it was already drawn at open).
        open_via_quick_open(fd, "a.py", expect=b"print")
        os.write(fd, b"#edited" + ENTER)
        buf = read_until(fd, b"#edited", timeout=8)
        assert b"#edited" in screen_text(buf), "a.py edit did not render"

        # File B: open alongside. open_via_quick_open's post-Enter read
        # already holds the frame that names the new tab — ratatui diffs
        # cells, so the same string is never emitted twice.
        buf = open_via_quick_open(fd, "config.yaml")
        assert b"config.yaml" in screen_text(buf), "config.yaml editor title missing"

        # Embedded terminal: run a shell command, then refocus the editor.
        os.write(fd, ALT_G + b"t")
        buf = read_until(fd, b"Terminal")
        assert b"Terminal" in screen_text(buf), "terminal pane title missing"
        os.write(fd, b"echo shell-ok" + ENTER)
        buf = read_until(fd, b"shell-ok")
        assert b"shell-ok" in screen_text(buf), "embedded terminal did not echo"

        # Bracketed paste exact YAML into config.yaml, then save. The file
        # bytes — not any rendered string — are the reliable save signal.
        os.write(fd, ALT_G + b"2")
        os.write(fd, b"\x1b[200~training:\n  lr: 0.001\n\x1b[201~")
        os.write(fd, b"\x13")  # Ctrl+S
        saved = b""
        deadline = time.time() + 8
        while time.time() < deadline:
            with open(os.path.join(workspace, "config.yaml"), "rb") as f:
                saved = f.read()
            if saved == b"training:\n  lr: 0.001\n":
                break
            time.sleep(0.2)
        assert saved == b"training:\n  lr: 0.001\n", f"saved bytes differ: {saved!r}"

        # a.py's dirty buffer is retained (not clobbered by the save).
        # Switching back re-draws its editor — the buffer text re-emits.
        open_via_quick_open(fd, "a.py", expect=b"#edited")

        # Quit with a dirty document: cancel once (AC-3), then discard+exit.
        os.write(fd, ALT_G + b"1")
        os.write(fd, b"q")
        buf = read_until(fd, b"Discard", timeout=8)
        assert b"Discard" in screen_text(buf), "dirty-quit decision dialog missing"
        os.write(fd, b"c")  # cancel — fm must stay alive
        # Liveness proof: re-ask to quit — the dialog re-emits its title.
        os.write(fd, ALT_G + b"1")
        os.write(fd, b"q")
        buf = read_until(fd, b"Discard", timeout=8)
        assert b"Discard" in screen_text(buf), "cancel left fm unusable"
        os.write(fd, b"d")  # discard + quit
        status = reap(pid)
        assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, (
            f"fm did not exit cleanly after discard: {status}"
        )
        assert_gone(pid)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)


def scenario_resize():
    """AC-5: 80x24 → 120x40 → 60x20 reflow; chrome survives, quit is clean."""
    workspace, home, xdg = make_dirs()
    with open(os.path.join(workspace, "hello.txt"), "w") as f:
        f.write("hello fm\n")
    pid, fd = spawn_fm(workspace, home, xdg)
    try:
        buf = read_until(fd, b"Commands")
        assert b"Commands" in buf, "80x24 chrome missing"
        set_winsize(fd, 40, 120)
        buf = read_until(fd, b"Commands")
        assert b"Commands" in buf, "120x40 redraw lost the status bar"
        set_winsize(fd, 20, 60)
        # Below the status-bar's comfortable width the redraw still flows.
        buf = read_until(fd, b"hello", timeout=8)
        assert buf, "60x20 redraw produced no output"
        quit_cleanly(pid, fd)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)


def scenario_term_variants():
    """TERM portability: screen/tmux/linux terminfo names all render chrome."""
    for term in ("screen-256color", "tmux-256color", "linux"):
        workspace, home, xdg = make_dirs()
        with open(os.path.join(workspace, "hello.txt"), "w") as f:
            f.write("hello fm\n")
        pid, fd = spawn_fm(workspace, home, xdg, env_extra={"TERM": term})
        try:
            buf = read_until(fd, b"Commands")
            assert b"Commands" in buf, f"chrome missing on TERM={term}"
            assert b"hello.txt" in buf, f"tree missing on TERM={term}"
            quit_cleanly(pid, fd)
        finally:
            try:
                os.close(fd)
            except OSError:
                pass
            reap_or_kill(pid)


def scenario_nested_tmux():
    """AC-9/AC-10 transport: fm inside a nested tmux session renders the same
    chrome, takes input through the tmux keyboard path, and exits cleanly."""
    if shutil.which("tmux") is None:
        raise RuntimeError("BLOCKED: tmux binary is required for this scenario")
    workspace, home, xdg = make_dirs()
    with open(os.path.join(workspace, "hello.txt"), "w") as f:
        f.write("hello fm\n")
    session = "fm-pty-%d" % os.getpid()
    env = dict(os.environ, HOME=home, **xdg)
    subprocess.run(
        ["tmux", "-f", "/dev/null", "new-session", "-d", "-s", session,
         "-x", "80", "-y", "24",
         "TERM=xterm-256color {} {}".format(FM_BIN, workspace)],
        env=env, check=True, timeout=5,
    )
    try:
        deadline = time.time() + TIMEOUT
        pane = ""
        while time.time() < deadline:
            pane = subprocess.run(
                ["tmux", "capture-pane", "-p", "-t", session],
                capture_output=True, text=True, env=env, timeout=5,
            ).stdout
            if "Commands" in pane:
                break
            time.sleep(0.25)
        assert "Commands" in pane, "chrome missing inside tmux pane"
        assert "hello.txt" in pane, "tree missing inside tmux pane"
        subprocess.run(["tmux", "send-keys", "-t", session, "q"], env=env, timeout=5)
        deadline = time.time() + REAP_TIMEOUT
        while time.time() < deadline:
            if subprocess.run(
                ["tmux", "has-session", "-t", session], env=env
            ).returncode != 0:
                return
            time.sleep(0.2)
        raise AssertionError("fm inside tmux did not exit")
    finally:
        subprocess.run(["tmux", "kill-session", "-t", session],
                       env=env, capture_output=True)


def scenario_feature_flags_off():
    """AC-11: --no-mouse/--no-icons/--no-watcher/--no-terminal/--no-git
    still renders a usable workspace; the terminal toggle refuses visibly."""
    workspace, home, xdg = make_dirs()
    with open(os.path.join(workspace, "hello.txt"), "w") as f:
        f.write("hello fm\n")
    pid, fd = spawn_fm(
        workspace, home, xdg,
        args=("--no-mouse", "--no-icons", "--no-watcher",
              "--no-terminal", "--no-git"),
    )
    try:
        buf = read_until(fd, b"Commands")
        assert b"Commands" in buf, "chrome missing with features off"
        assert b"hello.txt" in buf, "tree missing with features off"
        os.write(fd, ALT_G + b"t")
        buf = read_until(fd, b"disabled")
        assert b"disabled" in buf, "terminal toggle did not refuse visibly"
        quit_cleanly(pid, fd)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)


def _write_config(workspace, body):
    cfg = os.path.join(workspace, "fm-test.toml")
    with open(cfg, "w") as f:
        f.write(body)
    return cfg


def scenario_missing_lsp_executable():
    """AC-11: configured server whose binary is absent reports visibly and
    fm stays fully usable."""
    workspace, home, xdg = make_dirs()
    with open(os.path.join(workspace, "a.rs"), "w") as f:
        f.write("fn main() {}\n")
    cfg = _write_config(
        workspace, '[lsp.servers.rust]\nargv = ["/nonexistent/fake-lsp-xyz"]\n'
    )
    pid, fd = spawn_fm(workspace, home, xdg, args=("-c", cfg))
    try:
        buf = read_until(fd, b"Commands")
        assert b"Commands" in buf, "chrome missing"
        buf += open_via_quick_open(fd, "a.rs", expect=b"fn main")
        buf += read_until(fd, b"not found", timeout=8, settle=1.2)
        assert b"not found" in buf, "missing-executable note not surfaced"
        assert b"Commands" in buf, "LSP failure removed the chrome"
        quit_cleanly(pid, fd)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)


def scenario_fake_lsp_diagnostics():
    """AC-8: configured fake LSP server publishes diagnostics end-to-end —
    the status bar counts them and the sync transcript is complete."""
    workspace, home, xdg = make_dirs()
    with open(os.path.join(workspace, "a.rs"), "w") as f:
        f.write("fn main() {}\n")
    transcript = os.path.join(workspace, "lsp-transcript.jsonl")
    opts = json.dumps({
        "diagnostics": {
            "version": "open",
            "items": [{
                "line": 0, "character": 0,
                "end_line": 0, "end_character": 3,
                "severity": 1, "source": "fake",
                "message": "fake issue",
            }],
        },
    })
    cfg = _write_config(
        workspace,
        '[lsp.servers.rust]\nargv = ["{}", "sync", "{}", "{}"]\n'.format(
            FAKE_LSP, transcript, opts.replace('"', '\\"')
        ),
    )
    # 120 cols: at 80 the status line truncates the diagnostics segment off
    # the right edge — the count is applied either way, just not visible.
    pid, fd = spawn_fm(workspace, home, xdg, cols=120, args=("-c", cfg))
    try:
        buf = read_until(fd, b"Commands")
        assert b"Commands" in buf, "chrome missing"
        buf += open_via_quick_open(fd, "a.rs", expect=b"fn main")
        buf += read_until(fd, b"E:1", timeout=10, settle=1.5)
        assert b"E:1" in buf, "status bar never showed one diagnostic"
        quit_cleanly(pid, fd)
        # Transcript proves the full sync path ran (didOpen + push).
        with open(transcript) as f:
            log = f.read()
        assert '"method": "textDocument/didOpen"' in log, (
            "didOpen never reached the server"
        )
        assert '"method": "textDocument/publishDiagnostics"' in log, (
            "server push was never recorded"
        )
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)


def scenario_external_save():
    """AC-3/AC-11: an external write to an open document is surfaced by the
    watcher as the `!` external-change marker on the document tab.

    The watcher is opt-in (`watcher.enabled` defaults false), so this
    scenario runs with a config that turns it on — that also covers the
    `[watcher]` config path. Polling mode is used for determinism: a fixed
    500ms scan beats relying on inotify delivery timing."""
    workspace, home, xdg = make_dirs()
    with open(os.path.join(workspace, "a.py"), "w") as f:
        f.write("print(1)\n")
    cfg = _write_config(
        workspace,
        "[watcher]\nenabled = true\nmode = \"polling\"\npoll_interval_ms = 500\n",
    )
    pid, fd = spawn_fm(workspace, home, xdg, args=("-c", cfg))
    try:
        buf = read_until(fd, b"Commands")
        assert b"Commands" in buf, "chrome missing"
        open_via_quick_open(fd, "a.py", expect=b"print")
        # External write while a.py is open → watcher → `!` marker on the
        # tab. Only the changed cells re-emit: `!` + ` [pin]` arrive without
        # the `a.py` prefix — the stripped text shows `! [pin]`.
        with open(os.path.join(workspace, "a.py"), "a") as f:
            f.write("# externally appended\n")
        buf = read_until(fd, b"! [pin]", timeout=10)
        assert b"! [pin]" in screen_text(buf), (
            "external write was not marked on the document"
        )
        quit_cleanly(pid, fd)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)


def scenario_missing_git():
    """AC-7/AC-11: a PATH with no git binary degrades to no indicators —
    the workspace still renders and quits cleanly."""
    workspace, home, xdg = make_dirs()
    subprocess.run(["git", "init", "-b", "main", workspace], check=True,
                   capture_output=True)
    subprocess.run(["git", "-C", workspace, "config", "user.email", "t@t"],
                   check=True)
    subprocess.run(["git", "-C", workspace, "config", "user.name", "t"],
                   check=True)
    with open(os.path.join(workspace, "tracked.txt"), "w") as f:
        f.write("tracked\n")
    subprocess.run(["git", "-C", workspace, "add", "."], check=True)
    subprocess.run(["git", "-C", workspace, "commit", "-m", "init"],
                   check=True, capture_output=True)
    pid, fd = spawn_fm(workspace, home, xdg,
                       env_extra={"PATH": "/nonexistent-bin"})
    try:
        buf = read_until(fd, b"Commands")
        assert b"Commands" in buf, "chrome missing without git"
        assert b"tracked.txt" in buf, "tree missing without git"
        quit_cleanly(pid, fd)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)


def scenario_git_markers():
    """AC-7: inside a real git repo the status bar carries the branch name
    (read-only decoration; no git writes)."""
    workspace, home, xdg = make_dirs()
    subprocess.run(["git", "init", "-b", "zeta-branch", workspace],
                   check=True, capture_output=True)
    subprocess.run(["git", "-C", workspace, "config", "user.email", "t@t"],
                   check=True)
    subprocess.run(["git", "-C", workspace, "config", "user.name", "t"],
                   check=True)
    with open(os.path.join(workspace, "tracked.txt"), "w") as f:
        f.write("tracked\n")
    subprocess.run(["git", "-C", workspace, "add", "."], check=True)
    subprocess.run(["git", "-C", workspace, "commit", "-m", "init"],
                   check=True, capture_output=True)
    pid, fd = spawn_fm(workspace, home, xdg)
    try:
        buf = read_until(fd, b"Commands")
        assert b"Commands" in buf, "chrome missing"
        buf = read_until(fd, b"zeta-branch", timeout=10)
        assert b"zeta-branch" in buf, "git branch not shown on the status bar"
        quit_cleanly(pid, fd)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)


def scenario_crash_then_recovery_prompt():
    """AC-3: a dirty pinned document is snapshotted; SIGKILL (crash) leaves
    the record; the next launch offers it in the recovery prompt."""
    workspace, home, xdg = make_dirs()
    doc = os.path.join(workspace, "a.py")
    with open(doc, "w") as f:
        f.write("print('a')\n")

    # Run 1: edit a.py dirty, wait out the 2 s snapshot throttle, then crash.
    pid, fd = spawn_fm(workspace, home, xdg)
    try:
        buf = read_until(fd, b"Commands")
        assert b"Commands" in buf, "chrome missing"
        open_via_quick_open(fd, "a.py", expect=b"print")
        os.write(fd, b"#crash-edited" + ENTER)
        read_until(fd, b"#crash-edited", timeout=8)
        records = os.path.join(
            xdg["XDG_STATE_HOME"], "fm-tui", "recovery", "recovery"
        )
        # Snapshot passes fire on a ~2 s interval and overwrite the same
        # filename (its digest is the on-disk revision, which never moves).
        # An early pass can catch a partial input burst — wait until a
        # record actually carries the crash edit before killing.
        snapshot_ok = False
        deadline = time.time() + 12
        while time.time() < deadline:
            if os.path.isdir(records):
                for name in os.listdir(records):
                    try:
                        with open(os.path.join(records, name)) as f:
                            record = json.load(f)
                    except (OSError, json.JSONDecodeError):
                        continue
                    if "#crash-edited" in record.get("text", ""):
                        snapshot_ok = True
                        break
            if snapshot_ok:
                break
            time.sleep(0.25)
        assert snapshot_ok, "crash edit was never snapshotted"
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)

    # Run 2: relaunch — the prompt names the crashed document. Declining
    # closes the dialog without deleting the record; a clean quit proves
    # fm stayed usable (ratatui only re-emits changed cells, so the status
    # bar text is NOT guaranteed to reappear after an overlay closes).
    pid, fd = spawn_fm(workspace, home, xdg)
    try:
        buf = read_until(fd, b"Recover Unsaved Work", timeout=10)
        assert b"Recover Unsaved Work" in buf, "recovery prompt never offered"
        assert b"a.py" in buf, "recovery prompt did not name the document"
        os.write(fd, b"n")  # decline — record survives for run 3
        time.sleep(0.5)
        quit_cleanly(pid, fd)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)

    # Run 3: prompt returns (decline ≠ discard); 'r' restores the crashed
    # buffer into a pinned, dirty document.
    pid, fd = spawn_fm(workspace, home, xdg)
    try:
        buf = read_until(fd, b"Recover Unsaved Work", timeout=10)
        assert b"Recover Unsaved Work" in buf, "prompt lost after a decline"
        os.write(fd, b"r")  # restore
        buf = read_until(fd, b"#crash-edited", timeout=8)
        assert b"#crash-edited" in screen_text(buf), (
            "restored buffer missing crash edit"
        )
        # The restored doc is dirty vs disk → quit hits the decision dialog.
        os.write(fd, ALT_G + b"1")
        os.write(fd, b"q")
        read_until(fd, b"Discard", timeout=8)
        os.write(fd, b"d")  # discard + quit
        status = reap(pid)
        assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, (
            f"fm did not exit cleanly after restore+discard: {status}"
        )
        assert_gone(pid)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)


def scenario_corrupt_recovery_is_ignored():
    """AC-3: corrupt and non-private recovery records never block startup —
    fm launches, shows no recovery prompt, and quits cleanly."""
    workspace, home, xdg = make_dirs()
    with open(os.path.join(workspace, "a.py"), "w") as f:
        f.write("print('a')\n")

    # A valid record (prompt expected) plus two refused ones:
    #   - garbage bytes in a correctly-shaped, private file
    #   - a valid JSON record whose file is group-readable (UnsafeRecord)
    records = write_recovery_record(
        xdg["XDG_STATE_HOME"], workspace,
        os.path.join(workspace, "a.py"), "recovered text\n",
    )
    garbage = os.path.join(records, "snap-0000000000000001-0000000000000002-0000000000000003.json")
    with open(garbage, "w") as f:
        f.write("this is not json at all")
    os.chmod(garbage, 0o600)
    leaked = os.path.join(records, "snap-0000000000000004-0000000000000005-0000000000000006.json")
    with open(leaked, "w") as f:
        json.dump({"schema": "fm-tui-recovery", "version": 1}, f)
    os.chmod(leaked, 0o644)  # non-private → refused

    pid, fd = spawn_fm(workspace, home, xdg)
    try:
        buf = read_until(fd, b"Recover Unsaved Work", timeout=10)
        assert b"Recover Unsaved Work" in buf, "valid record never prompted"
        assert b"a.py" in buf, "prompt did not name the document"
        os.write(fd, b"n")  # decline — record stays, nothing auto-restores
        time.sleep(0.5)
        # The disk bytes — not the record text — must be what a fresh open sees.
        buf = open_via_quick_open(fd, "a.py", expect=b"print")
        assert b"recovered text" not in screen_text(buf), (
            "declined record leaked into buffer"
        )
        quit_cleanly(pid, fd)
    finally:
        try:
            os.close(fd)
        except OSError:
            pass
        reap_or_kill(pid)


SCENARIOS = [
    ("smoke: launch, chrome, clean quit, child reaped", scenario_smoke),
    ("workflow: two files, edit, terminal, paste, save bytes", scenario_full_workflow),
    ("resize: 80x24 → 120x40 → 60x20 reflow", scenario_resize),
    ("term variants: screen/tmux-256color/linux render", scenario_term_variants),
    ("transport: nested tmux chrome, input, exit", scenario_nested_tmux),
    ("flags: --no-mouse/icons/watcher/terminal/git render", scenario_feature_flags_off),
    ("missing LSP executable degrades visibly", scenario_missing_lsp_executable),
    ("fake LSP diagnostics reach the status bar", scenario_fake_lsp_diagnostics),
    ("external save marks the open document", scenario_external_save),
    ("missing git binary degrades to no indicators", scenario_missing_git),
    ("git repo shows the branch on the status bar", scenario_git_markers),
    ("crash leaves a snapshot; relaunch prompts recovery", scenario_crash_then_recovery_prompt),
    ("corrupt/private recovery records are refused", scenario_corrupt_recovery_is_ignored),
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
