//! Instance console sessions (owner decisions D38, D39).
//!
//! `stream_vm_console` runs `incus console <instance>` on a pseudo-terminal:
//! util-linux `setsid --ctty` makes the terminal the console's controlling
//! one, so the console gets SIGHUP whenever the master side closes, however
//! the agent ends. A reader thread keeps the newest 64 KiB of output, which
//! `send_console_input` hands back by pull. One session per instance.

use crate::config::Env;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// D39: the newest console output kept per session.
pub const BUFFER: usize = 64 * 1024;
/// How long a console that exits at once counts as one that could not start.
const START_GRACE: Duration = Duration::from_millis(500);
/// How long `send_console_input` waits for the console to answer.
const ANSWER_WAIT: Duration = Duration::from_millis(500);
const SETTLE: Duration = Duration::from_millis(150);

#[derive(Default)]
pub(crate) struct Output {
    pub(crate) pending: Vec<u8>,
    /// The console process has exited (and is about to be, or has been,
    /// reaped): it must not be signalled any more.
    pub(crate) exited: bool,
}

struct Session {
    vm: String,
    pid: i32,
    master: Mutex<std::fs::File>,
    output: Arc<Mutex<Output>>,
    reader: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Session {
    fn exited(&self) -> bool {
        self.output.lock().unwrap().exited
    }

    /// Ends the console process and waits for the reader to reap it.
    fn close(&self) {
        {
            let output = self.output.lock().unwrap();
            if !output.exited {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(self.pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
        }
        if let Some(reader) = self.reader.lock().unwrap().take() {
            let _ = reader.join();
        }
    }

    fn take(&self) -> (String, bool) {
        drain(&mut self.output.lock().unwrap())
    }
}

/// The output since the last read, and whether the process has exited. An
/// incomplete UTF-8 sequence at the end waits for its remaining bytes while
/// the console still runs.
pub(crate) fn drain(output: &mut Output) -> (String, bool) {
    let keep = if output.exited {
        0
    } else {
        incomplete_tail(&output.pending)
    };
    let tail = output.pending.split_off(output.pending.len() - keep);
    let text = String::from_utf8_lossy(&std::mem::replace(&mut output.pending, tail)).into_owned();
    (text, output.exited)
}

/// The length of a trailing UTF-8 sequence that more bytes could complete.
fn incomplete_tail(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(3) {
        let lead = bytes[bytes.len() - back];
        if lead & 0xC0 == 0x80 {
            continue;
        }
        let needed = match lead {
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => 1,
        };
        return if needed > back { back } else { 0 };
    }
    0
}

/// Appends console output, keeping the newest `BUFFER` bytes, never starting
/// inside a UTF-8 sequence.
pub(crate) fn append(pending: &mut Vec<u8>, chunk: &[u8]) {
    pending.extend_from_slice(chunk);
    if pending.len() > BUFFER {
        let mut drop = pending.len() - BUFFER;
        while drop < pending.len() && (pending[drop] & 0xC0) == 0x80 {
            drop += 1;
        }
        pending.drain(..drop);
    }
}

/// Every console session this agent runs.
#[derive(Default)]
pub struct Consoles {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

fn winsize(width: u16, height: u16) -> rustix::termios::Winsize {
    rustix::termios::Winsize {
        ws_row: height,
        ws_col: width,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// A terminal size from a JSON number: a whole number of cells, 1 to 65535.
pub fn cells(value: f64) -> Option<u16> {
    (1.0..=f64::from(u16::MAX)).contains(&value).then_some(value as u16)
}

fn open_pty(size: Option<(u16, u16)>) -> Result<(OwnedFd, std::fs::File), String> {
    use rustix::pty::{grantpt, openpt, ptsname, unlockpt, OpenptFlags};
    use std::os::unix::fs::OpenOptionsExt;
    let fail = |e: rustix::io::Errno| format!("open console terminal: {e}");
    let master =
        openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC).map_err(fail)?;
    grantpt(&master).map_err(fail)?;
    unlockpt(&master).map_err(fail)?;
    let name = ptsname(&master, Vec::new()).map_err(fail)?;
    // O_NOCTTY: the agent may be a session leader, and must not take the
    // console's terminal as its own.
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_NOCTTY)
        .open(std::ffi::OsStr::new(&*name.to_string_lossy()))
        .map_err(|e| format!("open console terminal: {e}"))?;
    if let Some((width, height)) = size {
        rustix::termios::tcsetwinsize(&master, winsize(width, height)).map_err(fail)?;
    }
    Ok((master, slave))
}

impl Consoles {
    fn get(&self, operation_id: &str) -> Result<Arc<Session>, String> {
        self.sessions
            .lock()
            .unwrap()
            .get(operation_id)
            .cloned()
            .ok_or_else(|| format!("console session {operation_id:?} not found"))
    }

    fn remove(&self, operation_id: &str) -> Option<Arc<Session>> {
        self.sessions.lock().unwrap().remove(operation_id)
    }

    /// D38: starts the console of `vm` under `operation_id`, closing any
    /// session for the same instance or with the same id first.
    pub fn start(
        &self,
        env: &Env,
        binary: &str,
        vm: &str,
        operation_id: &str,
        size: Option<(u16, u16)>,
    ) -> Result<(), String> {
        let stale: Vec<Arc<Session>> = {
            let mut sessions = self.sessions.lock().unwrap();
            let ids: Vec<String> = sessions
                .iter()
                .filter(|(id, s)| s.vm == vm || id.as_str() == operation_id)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| sessions.remove(id)).collect()
        };
        for session in stale {
            session.close();
        }
        let setsid = crate::hostobs::look_path(env, "setsid")
            .ok_or("the console needs util-linux setsid, which this host does not have")?;
        let (master, slave) = open_pty(size)?;
        let stdio = || slave.try_clone().map(Stdio::from);
        let spawned = (|| {
            Command::new(&setsid)
                .args(["--ctty", binary, "--project", "default", "console", vm])
                .env_clear()
                .envs(env.pairs())
                .stdin(stdio()?)
                .stdout(stdio()?)
                .stderr(stdio()?)
                .spawn()
        })();
        drop(slave);
        let mut child = spawned.map_err(|e| format!("start console for {vm:?}: {e}"))?;
        let pid = child.id() as i32;
        let master = std::fs::File::from(master);
        let output = Arc::new(Mutex::new(Output::default()));
        let reader = {
            let mut master = master.try_clone().map_err(|e| e.to_string())?;
            let output = output.clone();
            std::thread::Builder::new()
                .name("console".into())
                .spawn(move || {
                    let mut chunk = [0u8; 4096];
                    // EIO once every slave side is closed: the console ended.
                    while let Ok(n @ 1..) = master.read(&mut chunk) {
                        append(&mut output.lock().unwrap().pending, &chunk[..n]);
                    }
                    use nix::sys::wait::{waitid, Id, WaitPidFlag};
                    let _ = waitid(
                        Id::Pid(nix::unistd::Pid::from_raw(pid)),
                        WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT,
                    );
                    output.lock().unwrap().exited = true;
                    let _ = child.wait();
                })
                .map_err(|e| e.to_string())?
        };
        let session = Arc::new(Session {
            vm: vm.to_string(),
            pid,
            master: Mutex::new(master),
            output,
            reader: Mutex::new(Some(reader)),
        });
        let started = Instant::now();
        while started.elapsed() < START_GRACE && !session.exited() {
            std::thread::sleep(Duration::from_millis(20));
        }
        if session.exited() {
            session.close();
            let (text, _) = session.take();
            let text = text.trim();
            return Err(format!(
                "console for {vm:?} could not start: {}",
                if text.is_empty() {
                    "the console exited"
                } else {
                    text
                }
            ));
        }
        self.sessions
            .lock()
            .unwrap()
            .insert(operation_id.to_string(), session);
        Ok(())
    }

    /// D39: writes `data`, waits briefly for the answer, and returns the
    /// output since the last read and whether the console has exited. The
    /// read that reports the exit is the session's last.
    pub fn send(&self, operation_id: &str, data: &str) -> Result<(String, bool), String> {
        let session = self.get(operation_id)?;
        if !data.is_empty() && !session.exited() {
            session
                .master
                .lock()
                .unwrap()
                .write_all(data.as_bytes())
                .map_err(|e| format!("write to console session {operation_id:?}: {e}"))?;
            let started = Instant::now();
            let answered = || {
                let output = session.output.lock().unwrap();
                !output.pending.is_empty() || output.exited
            };
            while started.elapsed() < ANSWER_WAIT && !answered() {
                std::thread::sleep(Duration::from_millis(20));
            }
            std::thread::sleep(SETTLE);
        }
        let (text, closed) = session.take();
        if closed {
            if let Some(session) = self.remove(operation_id) {
                session.close();
            }
        }
        Ok((text, closed))
    }

    pub fn resize(&self, operation_id: &str, width: u16, height: u16) -> Result<(), String> {
        let session = self.get(operation_id)?;
        let master = session.master.lock().unwrap();
        rustix::termios::tcsetwinsize(master.as_fd(), winsize(width, height))
            .map_err(|e| format!("resize console session {operation_id:?}: {e}"))
    }

    /// Ends every session: the agent is stopping.
    pub fn close_all(&self) {
        let sessions: Vec<Arc<Session>> = self
            .sessions
            .lock()
            .unwrap()
            .drain()
            .map(|(_, s)| s)
            .collect();
        for session in sessions {
            session.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D39: the buffer keeps the newest 64 KiB and never starts inside a
    /// UTF-8 sequence.
    #[test]
    fn buffer_keeps_the_newest_output() {
        let mut pending = Vec::new();
        append(&mut pending, &vec![b'a'; BUFFER]);
        append(&mut pending, "é-end".as_bytes());
        assert!(pending.len() <= BUFFER);
        assert!(pending.ends_with(b"-end"));
        assert!(std::str::from_utf8(&pending).is_ok());
        let mut pending = vec![b'x'; BUFFER - 1];
        append(&mut pending, "é".as_bytes());
        assert!(
            std::str::from_utf8(&pending).is_ok(),
            "a split character is dropped whole"
        );
        assert_eq!(incomplete_tail("ab\u{e9}".as_bytes()), 0);
        assert_eq!(incomplete_tail(&"ab\u{e9}".as_bytes()[..3]), 1);
        assert_eq!(
            incomplete_tail(b"\xff ok\xe2\x82"),
            2,
            "an earlier invalid byte does not matter"
        );
        assert_eq!(incomplete_tail(b"ok\xe2\x82\xac"), 0);
        assert_eq!(cells(0.0), None);
        assert_eq!(cells(-3.0), None);
        assert_eq!(cells(120.0), Some(120));
        assert_eq!(cells(70000.0), None);
    }

    /// A terminal program stands in for `incus console`: the session reads
    /// its output, writes to it, resizes it, and ends it with no process
    /// left behind.
    #[test]
    fn a_session_runs_reads_and_ends() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("incus");
        // `incus --project default console <vm>`: print the size, then echo
        // lines until `quit`.
        std::fs::write(
            &fake,
            "#!/bin/sh\necho \"console $4 $(stty size)\"\nwhile read line; do [ \"$line\" = quit ] && exit 0; echo \"got:$line\"; done\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let env = Env::from_pairs([("PATH", "/usr/bin:/bin")]);
        let consoles = Consoles::default();
        consoles
            .start(
                &env,
                fake.to_str().unwrap(),
                "v1",
                "console-v1",
                Some((100, 30)),
            )
            .unwrap();
        let (text, closed) = consoles.send("console-v1", "").unwrap();
        assert!(text.contains("console v1 30 100"), "{text:?}");
        assert!(!closed);
        consoles.resize("console-v1", 120, 40).unwrap();
        let (text, _) = consoles.send("console-v1", "hello\n").unwrap();
        assert!(text.contains("got:hello"), "{text:?}");
        // A second start for the instance replaces the first session.
        let first = consoles.get("console-v1").unwrap().pid;
        consoles
            .start(&env, fake.to_str().unwrap(), "v1", "other", None)
            .unwrap();
        assert!(consoles.get("console-v1").is_err());
        assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(first), None).is_err());
        let (_, closed) = consoles.send("other", "quit\n").unwrap();
        assert!(closed);
        assert!(consoles
            .send("other", "")
            .unwrap_err()
            .contains("not found"));
        consoles
            .start(&env, fake.to_str().unwrap(), "v2", "console-v2", None)
            .unwrap();
        let pid = consoles.get("console-v2").unwrap().pid;
        consoles.close_all();
        assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err());
    }

    /// A console that exits at once could not start, and leaves nothing.
    #[test]
    fn a_console_that_exits_at_once_is_an_error() {
        let env = Env::from_pairs([("PATH", "/usr/bin:/bin")]);
        let consoles = Consoles::default();
        let error = consoles
            .start(&env, "/bin/false", "v1", "console-v1", None)
            .unwrap_err();
        assert!(error.contains("could not start"), "{error}");
        assert!(consoles.get("console-v1").is_err());
    }
}
