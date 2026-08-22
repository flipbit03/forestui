//! The terminal input modes forestui turns on, and every way out of the
//! process that has to turn them off again.
//!
//! These modes live in the *terminal*, not in the process, so nothing resets
//! them when forestui dies without getting to its cleanup: a pane whose
//! application is gone can be left reporting every pointer movement, on the
//! alternate screen, in raw mode. Inside tmux that is the pane's own state;
//! forestui started from a shell prompt hands it straight back to that shell.
//!
//! Defect A of issue #51 was that the OFF string lived on exactly one branch
//! of `run()`. It is now emitted from four places, each covering what the
//! others cannot:
//!
//! 1. [`ModeGuard`]'s `Drop` — normal exit, an early `?`, and any unwinding
//!    panic.
//! 2. A chained panic hook — the same panic, but also under `panic = "abort"`,
//!    where nothing is dropped. `ratatui`'s own hook restores the *screen* and
//!    knows nothing about these modes, so after a panic the display looked
//!    perfectly fine while all five modes were still set.
//! 3. A signal handler — `SIGTERM`/`SIGHUP` from a killed pane or window,
//!    where neither of the above runs.
//! 4. The next launch — `SIGKILL` can never be handled, so startup emits OFF
//!    before ON and heals whatever the previous run left behind.
//!
//! None of this explains the terminal wedge issue #51 was opened for: that
//! happens while forestui is *running*, so nothing has exited and nothing has
//! leaked. This module fixes the defect, not that.

use std::io::Write;

/// Every mode forestui can ask for: mouse buttons, drag, any-motion, SGR
/// coordinates, focus. [`modes_on`] narrows it to what a given run wants.
///
/// `?1003h` (any-motion) is what makes hover possible at all: without it the
/// terminal never reports a bare move, so no control can light up under the
/// pointer. It was previously left off because every motion report woke the
/// loop and repainted, which read as the app flickering under a moving mouse.
/// That is fixed at the source instead — `App::handle_mouse` only marks the
/// frame dirty when the *hovered target changes*, so crossing a control costs
/// one repaint and sliding around inside it costs none.
///
/// `?1006h` asks for SGR coordinates so columns past 223 still resolve.
///
/// `?1004h` asks for focus reporting, and without it the terminal never sends
/// focus in/out at all — so `Event::FocusGained` never arrived and everything
/// hung off it was dead code. `ensure_focus_events` turning on tmux's
/// `focus-events` is only half of the handshake: tmux forwards the sequences
/// to a pane that asked for them, and this is the asking.
///
/// Kept as one string so a test can pin it: a mode dropped from here fails
/// silently and takes a whole feature with it, which is how focus reporting
/// went missing and left `Event::FocusGained` unable to fire at all. A test
/// also pins the reverse — every mode set here has a matching reset in
/// [`MODES_OFF`], because a mode with no way off is the defect this module
/// exists for.
pub const MODES_ON: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?1004h";

/// The same modes, unset, in reverse order.
///
/// Always the *whole* set, whatever [`InputModes`] asked for: it doubles as
/// the self-heal, and a run that left a mode on is exactly the run whose flags
/// we cannot know.
pub const MODES_OFF: &str = "\x1b[?1004l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

/// The two modes that are optional, because each one costs something inside
/// tmux beyond the feature it buys.
///
/// A mouse or focus report that reaches tmux between the prefix key and the
/// next key cancels the command — tmux looks the report up in the `prefix`
/// key table, finds nothing, and falls back to the root table. That is
/// ordinary tmux behaviour with `mouse on`, but these two modes widen what
/// counts as a report: `?1003h` makes a bare pointer *move* one, and
/// `?1004h` (with tmux's `focus-events`) makes every focus change one. Neither
/// is bindable in tmux 3.4, so a user who trips over it has no config-side
/// defence — hence the flags (issue #51).
const MOTION_ON: &str = "\x1b[?1003h";
const FOCUS_ON: &str = "\x1b[?1004h";

/// Which optional input modes this run asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputModes {
    /// `?1003h`: any-motion reporting, which is what hover highlighting needs.
    pub hover: bool,
    /// `?1004h` plus tmux's `focus-events`: refreshing when the user returns.
    pub focus: bool,
}

impl Default for InputModes {
    fn default() -> Self {
        Self {
            hover: true,
            focus: true,
        }
    }
}

/// [`MODES_ON`] with the modes this run declined removed.
pub fn modes_on(modes: InputModes) -> String {
    let mut sequence = MODES_ON.to_string();
    if !modes.hover {
        sequence = sequence.replace(MOTION_ON, "");
    }
    if !modes.focus {
        sequence = sequence.replace(FOCUS_ON, "");
    }
    sequence
}

/// What a launch writes before it draws anything: OFF, then ON.
///
/// The leading OFF is the self-heal. `SIGKILL`, a pulled power cord and a
/// crashed terminal emulator can all leave the modes set with no chance to
/// clean up, and this makes that state cost one launch instead of being sticky
/// until the user finds the magic `printf`. It is also what makes the flags
/// honest: a run started with `--no-hover` turns motion reporting *off* even
/// if the previous run left it on.
pub fn startup_sequence(modes: InputModes) -> String {
    format!("{MODES_OFF}{}", modes_on(modes))
}

/// Owns the input modes for as long as it is alive.
///
/// Generic over the sink purely so a test can watch what it emits; the app
/// hands it `std::io::stdout()`.
pub struct ModeGuard<W: Write> {
    out: W,
}

impl<W: Write> ModeGuard<W> {
    /// The return value *is* the feature: dropping it turns the modes off, so
    /// a call whose guard is not held for the life of the UI turns them
    /// straight back off again.
    #[must_use]
    pub fn enable(mut out: W, modes: InputModes) -> Self {
        let _ = out.write_all(startup_sequence(modes).as_bytes());
        let _ = out.flush();
        Self { out }
    }
}

impl<W: Write> Drop for ModeGuard<W> {
    fn drop(&mut self) {
        let _ = self.out.write_all(MODES_OFF.as_bytes());
        let _ = self.out.flush();
    }
}

/// Emit OFF on a panic, then run whatever hook was already installed.
///
/// Call this *after* `ratatui::init()`, so this hook runs first and the modes
/// are reset before the screen is restored. Chaining rather than replacing is
/// the point: `ratatui`'s hook is what leaves the alternate screen and takes
/// the terminal out of raw mode, and dropping it would trade one silent
/// breakage for another.
pub fn chain_panic_hook() {
    install_panic_hook(|| {
        let mut out = std::io::stdout();
        let _ = out.write_all(MODES_OFF.as_bytes());
        let _ = out.flush();
    });
}

fn install_panic_hook(emit: fn()) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        emit();
        previous(info);
    }));
}

/// Reset the terminal from a signal handler, then die of the signal.
///
/// Must be installed before `ratatui::init()` enables raw mode: it snapshots
/// the terminal settings it finds, and those are the ones it puts back.
#[cfg(unix)]
pub fn install_signal_handlers() {
    signals::install();
}

#[cfg(not(unix))]
pub fn install_signal_handlers() {}

#[cfg(unix)]
mod signals {
    use std::cell::UnsafeCell;
    use std::mem::MaybeUninit;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The terminal settings from before raw mode, for the handler to restore.
    ///
    /// A plain `static` because a signal handler cannot take a lock: it may
    /// run on any thread, at any instruction, including one inside that same
    /// lock. Only `install` writes it, once, before any handler exists.
    struct SavedTermios(UnsafeCell<MaybeUninit<libc::termios>>);

    // SAFETY: written once by `install` before the first handler is armed and
    // only read afterwards.
    unsafe impl Sync for SavedTermios {}

    /// Leave the alternate screen and show the cursor.
    ///
    /// `ratatui::restore()` does this on every path that unwinds; nothing does
    /// it when a signal kills us mid-frame, and forestui started from a shell
    /// prompt inside a tmux window leaves that shell behind — on the alternate
    /// screen, with no cursor, in raw mode. Only the handler needs it, which is
    /// why it is not part of `MODES_OFF`.
    const SCREEN_RESTORE: &str = "\x1b[?1049l\x1b[?25h";

    static SAVED: SavedTermios = SavedTermios(UnsafeCell::new(MaybeUninit::uninit()));
    static HAVE_SAVED: AtomicBool = AtomicBool::new(false);

    /// The signals that end forestui in practice: a killed pane or window, a
    /// closed terminal, a `kill` from a shell, an `abort()`.
    ///
    /// `SIGKILL` and `SIGSTOP` are absent because they cannot be caught, which
    /// is what the self-heal on the next launch is for. `SIGSEGV` and `SIGBUS`
    /// are absent deliberately: Rust installs its own handler for those on an
    /// *alternate stack*, which is what prints "has overflowed its stack".
    /// Replacing it would trade that diagnostic for nothing — a handler
    /// without an alternate stack cannot run on an overflowed one anyway.
    const FATAL: [libc::c_int; 5] = [
        libc::SIGHUP,
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGTERM,
        libc::SIGABRT,
    ];

    pub fn install() {
        // SAFETY: a plain `tcgetattr` into local storage; failure (not a tty)
        // leaves `HAVE_SAVED` false and the handler simply skips the restore.
        unsafe {
            let mut current = MaybeUninit::<libc::termios>::uninit();
            if libc::tcgetattr(libc::STDIN_FILENO, current.as_mut_ptr()) == 0 {
                *SAVED.0.get() = current;
                HAVE_SAVED.store(true, Ordering::SeqCst);
            }
        }

        for signal in FATAL {
            // SAFETY: `action` is fully initialised below and the handler is a
            // valid `extern "C"` function for the lifetime of the process.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = handle_fatal as *const () as libc::sighandler_t;
                libc::sigemptyset(&mut action.sa_mask);
                // SA_RESETHAND puts the default disposition back on entry, so
                // the `raise` below kills the process with the signal it was
                // sent — the exit status stays honest and a SIGSEGV still
                // dumps core.
                action.sa_flags = libc::SA_RESETHAND;
                libc::sigaction(signal, &action, std::ptr::null_mut());
            }
        }
    }

    /// Async-signal-safe by construction: `write`, `tcsetattr` and `raise` are
    /// all on the POSIX list, and nothing here allocates or locks.
    extern "C" fn handle_fatal(signal: libc::c_int) {
        write_all(super::MODES_OFF.as_bytes());
        write_all(SCREEN_RESTORE.as_bytes());

        if HAVE_SAVED.load(Ordering::SeqCst) {
            // SAFETY: `SAVED` was initialised before this handler was armed.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, (*SAVED.0.get()).as_ptr());
            }
        }

        // SAFETY: re-raising with the default disposition restored above.
        unsafe {
            libc::raise(signal);
        }
    }

    /// `write(2)` straight at the file descriptor, looping over short writes.
    /// `print!` would take the stdout lock, which is exactly what a handler
    /// must not do.
    fn write_all(mut bytes: &[u8]) {
        while !bytes.is_empty() {
            // SAFETY: writing `bytes.len()` bytes from a slice we hold.
            let written = unsafe {
                libc::write(
                    libc::STDOUT_FILENO,
                    bytes.as_ptr().cast(),
                    bytes.len() as libc::size_t,
                )
            };
            if written <= 0 {
                // EINTR is the only error worth retrying, and a handler that
                // spins on a closed terminal is worse than a missed reset.
                return;
            }
            bytes = &bytes[written as usize..];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// A sink the test can still read after the guard that owns it is dropped.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<u8>>>);

    impl Recorder {
        fn written(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
        }
    }

    impl Write for Recorder {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Both of these are load-bearing and neither fails loudly.
    ///
    /// Without `?1003h` the terminal never reports a bare pointer move, so
    /// hover cannot work. Without `?1004h` it never reports focus at all, so
    /// `Event::FocusGained` never arrives and everything hanging off it —
    /// refreshing sessions and worktrees when the user comes back — is dead
    /// code that still compiles and still has passing tests.
    #[test]
    fn the_terminal_modes_we_depend_on_are_requested() {
        for (mode, why) in [
            ("?1003h", "any-motion, for hover"),
            ("?1004h", "focus reporting, for refresh on return"),
            ("?1006h", "SGR coordinates, for columns past 223"),
        ] {
            assert!(MODES_ON.contains(mode), "{mode} ({why}) is not requested");
            let off = mode.replace('h', "l");
            assert!(
                MODES_OFF.contains(&off),
                "{mode} ({why}) is requested but never turned off"
            );
        }
    }

    /// The general form of the above: a mode added to ON with no counterpart
    /// in OFF is a new leak, whatever it is called.
    #[test]
    fn every_mode_we_turn_on_has_a_way_off() {
        let modes: Vec<&str> = MODES_ON
            .split("\x1b[")
            .filter(|part| !part.is_empty())
            .collect();
        assert_eq!(modes.len(), 5, "MODES_ON no longer parses as we expect");

        for mode in modes {
            let off = mode.replace('h', "l");
            assert!(
                MODES_OFF.contains(&off),
                "\x1b[{mode} is set but \x1b[{off} is never sent"
            );
        }
    }

    /// The self-heal. A terminal a `SIGKILL`ed run left stranded is fixed by
    /// the next launch, so the bug costs one restart rather than a printf the
    /// user has to be told about.
    #[test]
    fn startup_turns_the_modes_off_before_turning_them_on() {
        let sequence = startup_sequence(InputModes::default());
        let off = sequence.find(MODES_OFF).expect("startup must reset first");
        let on = sequence.find(MODES_ON).expect("startup must set the modes");
        assert!(off < on, "startup set the modes before resetting them");
    }

    /// Declining a mode drops that request and nothing else — and never
    /// narrows the reset, which has to cover whatever a previous run set.
    #[test]
    fn declining_a_mode_drops_only_that_request() {
        let without_hover = modes_on(InputModes {
            hover: false,
            ..InputModes::default()
        });
        assert!(
            !without_hover.contains("?1003h"),
            "hover survived --no-hover"
        );
        for kept in ["?1000h", "?1002h", "?1006h", "?1004h"] {
            assert!(
                without_hover.contains(kept),
                "--no-hover also dropped {kept}"
            );
        }

        let without_focus = modes_on(InputModes {
            focus: false,
            ..InputModes::default()
        });
        assert!(
            !without_focus.contains("?1004h"),
            "focus survived --no-focus-events"
        );
        assert!(
            without_focus.contains("?1003h"),
            "--no-focus-events took hover"
        );

        let neither = modes_on(InputModes {
            hover: false,
            focus: false,
        });
        assert_eq!(neither, "\x1b[?1000h\x1b[?1002h\x1b[?1006h");
        assert!(
            startup_sequence(InputModes {
                hover: false,
                focus: false,
            })
            .starts_with(MODES_OFF),
            "a narrowed run must still reset every mode first"
        );
    }

    #[test]
    fn dropping_the_guard_turns_the_modes_off() {
        let recorder = Recorder::default();
        drop(ModeGuard::enable(recorder.clone(), InputModes::default()));

        let written = recorder.written();
        assert!(written.ends_with(MODES_OFF), "the guard left the modes on");
    }

    /// The path `ratatui`'s panic hook leaves uncovered: the screen is
    /// restored, and without this the modes are not.
    #[test]
    fn the_guard_turns_the_modes_off_when_the_frame_unwinds() {
        let recorder = Recorder::default();
        let held = recorder.clone();
        let result = std::panic::catch_unwind(move || {
            let _guard = ModeGuard::enable(held, InputModes::default());
            panic!("the event loop fell over");
        });

        assert!(result.is_err(), "the panic did not propagate");
        assert!(
            recorder.written().ends_with(MODES_OFF),
            "an unwinding panic left the modes on"
        );
    }

    /// `Drop` does not run under `panic = "abort"`, so the hook has to emit the
    /// reset itself — and still hand over to `ratatui`'s hook, which is what
    /// leaves the alternate screen.
    ///
    /// This swaps the process-wide panic hook for the length of the test. No
    /// other test panics deliberately, so the only thing it can swallow is the
    /// message from a test that is already failing.
    #[test]
    fn the_panic_hook_resets_the_modes_and_still_runs_the_previous_hook() {
        static RESET: AtomicBool = AtomicBool::new(false);
        static PREVIOUS_RAN: AtomicBool = AtomicBool::new(false);

        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| PREVIOUS_RAN.store(true, Ordering::SeqCst)));
        install_panic_hook(|| RESET.store(true, Ordering::SeqCst));

        let _ = std::panic::catch_unwind(|| panic!("boom"));
        std::panic::set_hook(original);

        assert!(RESET.load(Ordering::SeqCst), "the hook left the modes on");
        assert!(
            PREVIOUS_RAN.load(Ordering::SeqCst),
            "the hook swallowed ratatui's screen restore"
        );
    }
}
