#!/usr/bin/env python3
"""Check that forestui hands the terminal back however it is killed.

`src/terminal.rs` turns on five input modes and has to turn them off again on
every exit the OS lets it observe. A unit test can prove the *strings* are
right and that the guard drops; it cannot prove the process actually emitted
them when a signal arrived, or that raw mode was undone, because both are
properties of a real terminal.

So this runs forestui on a private pty, captures every byte it writes there,
kills it in each of the ways that used to leak (issue #51), and asserts the
reset arrived. Run it after touching anything in `src/terminal.rs`:

    scripts/terminal-modes-probe.py [path-to-forestui]

Isolation: the child gets its own throwaway forest directory, `TMUX_TMPDIR`
pointing at an empty directory, and `TMUX` set so it does not re-execute into
tmux. Any tmux command it runs can therefore only reach a server inside that
throwaway directory — never the one the user is working in.
"""

import fcntl
import os
import pty
import select
import signal
import subprocess
import sys
import shutil
import tempfile
import termios
import time

MODES_ON = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?1004h"
MODES_OFF = "\x1b[?1004l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l"
ALT_SCREEN_OFF = "\x1b[?1049l"

DEFAULT_BINARY = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "target",
    "release",
    "forestui",
)


class Run:
    """One forestui process on a pty of its own."""

    def __init__(self, forest, tmux_dir, extra_args=()):
        self.master, slave = pty.openpty()

        def preexec():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

        env = dict(
            os.environ,
            TERM="xterm-256color",
            TMUX=os.path.join(tmux_dir, "no-such-socket,0,0"),
            TMUX_TMPDIR=tmux_dir,
        )
        self.proc = subprocess.Popen(
            [BINARY, "--no-self-update", *extra_args, forest],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            preexec_fn=preexec,
            env=env,
            close_fds=True,
        )
        os.close(slave)
        self.written = ""

    def drain(self, seconds):
        deadline = time.time() + seconds
        while time.time() < deadline:
            ready, _, _ = select.select([self.master], [], [], 0.05)
            if not ready:
                continue
            try:
                chunk = os.read(self.master, 65536)
            except OSError:
                break
            if not chunk:
                break
            self.written += chunk.decode("utf8", "replace")

    def stop(self, how):
        how(self.proc)
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
        self.drain(1.0)

    def echo_restored(self):
        """Raw mode is terminal state too — a killed forestui that does not put
        the settings back leaves the shell behind it with no echo."""
        return bool(termios.tcgetattr(self.master)[3] & termios.ECHO)

    def after_the_modes_were_set(self):
        """Everything written after the last time the modes went on."""
        if MODES_ON not in self.written:
            return self.written
        return self.written[self.written.rindex(MODES_ON) + len(MODES_ON) :]

    def close(self):
        os.close(self.master)


results = []


def check(name, ok, detail=""):
    results.append(ok)
    print(f"{'PASS' if ok else 'FAIL'}  {name}{' -- ' + detail if detail else ''}")


SCRATCH = []


def start(extra_args=()):
    forest = tempfile.mkdtemp(prefix="forestui-probe-forest.")
    tmux_dir = tempfile.mkdtemp(prefix="forestui-probe-tmux.")
    SCRATCH.extend((forest, tmux_dir))
    run = Run(forest, tmux_dir, extra_args)
    run.drain(2.0)
    return run


def main():
    # SIGKILL cannot be caught, so the next launch is what heals the terminal:
    # startup resets every mode before asking for any.
    run = start()
    first = run.written
    run.stop(lambda p: p.send_signal(signal.SIGKILL))
    run.close()
    reset_at = first.find(MODES_OFF)
    set_at = first.find(MODES_ON)
    check(
        "startup resets the modes before setting them",
        0 <= reset_at < set_at,
        f"reset at {reset_at}, set at {set_at}",
    )

    for name, sig in [
        ("SIGTERM", signal.SIGTERM),
        ("SIGHUP", signal.SIGHUP),
        ("SIGINT", signal.SIGINT),
    ]:
        run = start()
        if MODES_ON not in run.written:
            check(f"{name}: forestui asked for the modes at all", False)
            run.close()
            continue
        run.stop(lambda p, s=sig: p.send_signal(s))
        tail = run.after_the_modes_were_set()
        check(f"{name}: modes off", MODES_OFF in tail)
        check(f"{name}: alternate screen left", ALT_SCREEN_OFF in tail)
        check(f"{name}: echo restored", run.echo_restored())
        check(
            f"{name}: exit status honest",
            run.proc.returncode == -sig,
            f"returncode {run.proc.returncode}",
        )
        run.close()

    # The path that always worked, so a fix cannot quietly break it.
    run = start()
    run.stop(lambda p: os.write(run.master, b"q"))
    check("normal quit: modes off", MODES_OFF in run.after_the_modes_were_set())
    check(
        "normal quit: alternate screen left",
        ALT_SCREEN_OFF in run.after_the_modes_were_set(),
    )
    run.close()

    print()
    print("all passed" if all(results) else "FAILURES PRESENT")
    return 0 if all(results) else 1


if __name__ == "__main__":
    BINARY = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_BINARY
    if not os.path.exists(BINARY):
        sys.exit(f"no forestui at {BINARY} — build it first, or pass its path")
    try:
        code = main()
    finally:
        for directory in SCRATCH:
            shutil.rmtree(directory, ignore_errors=True)
    sys.exit(code)
