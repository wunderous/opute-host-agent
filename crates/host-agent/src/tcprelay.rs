//! Go's `internal/tcprelay`: a plain TCP forwarder that accepts on a local
//! listener and pipes each connection to a target address. The cluster
//! domain forwards a guest bridge port with it (and postgres a SQL connector
//! port, M8e).

use std::collections::HashMap;
use std::io;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};

/// One live forwarder.
struct Session {
    listen_port: u16,
    listener: Arc<TcpListener>,
    active: Arc<Mutex<Vec<TcpStream>>>,
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<String, Session>,
    port_to_session: HashMap<u16, String>,
}

#[derive(Default)]
pub struct Manager {
    inner: Mutex<Inner>,
}

impl Manager {
    /// `Manager.Start`: listen on `listen_host:listen_port` and forward each
    /// connection to the target.
    pub fn start(
        &self,
        session_id: &str,
        listen_host: &str,
        listen_port: u16,
        target_host: &str,
        target_port: u16,
    ) -> Result<(), String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("sessionId is required".into());
        }
        let target_host = target_host.trim().to_string();
        if target_host.is_empty() {
            return Err("targetHost is required".into());
        }
        let listen_host = if listen_host.is_empty() {
            "0.0.0.0"
        } else {
            listen_host
        };
        let mut inner = self.inner.lock().unwrap();
        if inner.sessions.contains_key(session_id) {
            return Err(format!(
                "TCP relay session '{session_id}' is already active"
            ));
        }
        if listen_port != 0 {
            if let Some(owner) = inner.port_to_session.get(&listen_port) {
                return Err(format!(
                    "TCP relay listen port {listen_port} is already in use by {owner}"
                ));
            }
        }
        let address = if listen_host.contains(':') {
            format!("[{listen_host}]:{listen_port}")
        } else {
            format!("{listen_host}:{listen_port}")
        };
        let listener = TcpListener::bind(&address)
            .map_err(|e| format!("listen tcp {address}: {}", crate::goerr::errno_text(&e)))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let session = Session {
            listen_port: port,
            listener: Arc::new(listener),
            active: Arc::new(Mutex::new(Vec::new())),
        };
        let (listener, active) = (session.listener.clone(), session.active.clone());
        std::thread::spawn(move || accept_loop(&listener, &active, &target_host, target_port));
        inner.port_to_session.insert(port, session_id.to_string());
        inner.sessions.insert(session_id.to_string(), session);
        Ok(())
    }

    /// `Manager.Stop`: close the listener and every live connection.
    pub fn stop(&self, session_id: &str) -> bool {
        let session = {
            let mut inner = self.inner.lock().unwrap();
            let Some(session) = inner.sessions.remove(session_id) else {
                return false;
            };
            inner.port_to_session.remove(&session.listen_port);
            session
        };
        for conn in session.active.lock().unwrap().drain(..) {
            let _ = conn.shutdown(Shutdown::Both);
        }
        // Shutting the listening socket down wakes the blocked accept.
        let _ = nix::sys::socket::shutdown(
            session.listener.as_raw_fd(),
            nix::sys::socket::Shutdown::Both,
        );
        true
    }

    /// `Manager.StopAll`.
    pub fn stop_all(&self) {
        let ids: Vec<String> = self
            .inner
            .lock()
            .unwrap()
            .sessions
            .keys()
            .cloned()
            .collect();
        for id in ids {
            self.stop(&id);
        }
    }
}

fn accept_loop(
    listener: &TcpListener,
    active: &Arc<Mutex<Vec<TcpStream>>>,
    target_host: &str,
    target_port: u16,
) {
    for client in listener.incoming() {
        let Ok(client) = client else { return };
        let (active, host) = (active.clone(), target_host.to_string());
        std::thread::spawn(move || {
            let _ = pipe(client, &active, &host, target_port);
        });
    }
}

/// `pipe`: copy both ways, half-closing each direction as it ends.
fn pipe(
    client: TcpStream,
    active: &Arc<Mutex<Vec<TcpStream>>>,
    target_host: &str,
    target_port: u16,
) -> io::Result<()> {
    let upstream = match TcpStream::connect((target_host, target_port)) {
        Ok(upstream) => upstream,
        Err(_) => return Ok(()),
    };
    {
        let mut tracked = active.lock().unwrap();
        tracked.push(client.try_clone()?);
        tracked.push(upstream.try_clone()?);
    }
    let copy = |mut from: TcpStream, mut to: TcpStream| {
        std::thread::spawn(move || {
            let _ = io::copy(&mut from, &mut to);
            let _ = to.shutdown(Shutdown::Write);
        })
    };
    let forward = copy(client.try_clone()?, upstream.try_clone()?);
    let backward = copy(upstream.try_clone()?, client.try_clone()?);
    let _ = forward.join();
    let _ = backward.join();
    let _ = upstream.shutdown(Shutdown::Both);
    let _ = client.shutdown(Shutdown::Both);
    let ours = |s: &TcpStream| {
        let (a, b) = (s.peer_addr().ok(), s.local_addr().ok());
        [&client, &upstream]
            .iter()
            .any(|c| c.peer_addr().ok() == a && c.local_addr().ok() == b)
    };
    active.lock().unwrap().retain(|s| !ours(s));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn forwards_and_refuses_duplicates() {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = upstream.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut conn, _) = upstream.accept().unwrap();
            let mut buf = [0u8; 4];
            conn.read_exact(&mut buf).unwrap();
            conn.write_all(&buf).unwrap();
        });
        let manager = Manager::default();
        manager
            .start("a", "127.0.0.1", 0, "127.0.0.1", target)
            .unwrap();
        let port = *manager
            .inner
            .lock()
            .unwrap()
            .port_to_session
            .keys()
            .next()
            .unwrap();
        assert_eq!(
            manager
                .start("a", "127.0.0.1", 0, "127.0.0.1", target)
                .unwrap_err(),
            "TCP relay session 'a' is already active"
        );
        assert_eq!(
            manager
                .start("b", "127.0.0.1", port, "127.0.0.1", target)
                .unwrap_err(),
            format!("TCP relay listen port {port} is already in use by a")
        );
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client.write_all(b"ping").unwrap();
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
        assert!(manager.stop("a"));
        assert!(!manager.stop("a"));
    }
}
