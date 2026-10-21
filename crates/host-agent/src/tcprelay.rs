//! Go's `internal/tcprelay`: a plain TCP forwarder that accepts on a local
//! listener and pipes each connection to a target address. The cluster
//! domain forwards a guest bridge port with it (and postgres a SQL connector
//! port, M8e).

use std::collections::HashMap;
use std::io;
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
};
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};

/// The address a started forwarder reports (Go's exported `Session` fields).
#[derive(Clone, Debug, PartialEq)]
pub struct Bound {
    pub listen_host: String,
    pub listen_port: i64,
    pub target_host: String,
    pub target_port: i64,
}

/// One live forwarder.
struct Session {
    listen_port: i64,
    listener: Arc<TcpListener>,
    active: Arc<Mutex<Vec<TcpStream>>>,
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<String, Session>,
    port_to_session: HashMap<i64, String>,
}

#[derive(Default)]
pub struct Manager {
    inner: Mutex<Inner>,
}

impl Manager {
    /// `Manager.Start`: listen on `listen_host:listen_port` and forward each
    /// connection to the target. The session reports the address it is bound
    /// to, as Go's does.
    pub fn start(
        &self,
        session_id: &str,
        listen_host: &str,
        listen_port: i64,
        target_host: &str,
        target_port: i64,
    ) -> Result<Bound, String> {
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
        // `fmt.Sprintf("%s:%d")`, not `JoinHostPort`: an IPv6 host must come
        // bracketed.
        let (listener, host, port) = listen(&format!("{listen_host}:{listen_port}"))?;
        let session = Session {
            listen_port: i64::from(port),
            listener: Arc::new(listener),
            active: Arc::new(Mutex::new(Vec::new())),
        };
        let (listener, active) = (session.listener.clone(), session.active.clone());
        let dial_host = target_host.clone();
        std::thread::spawn(move || accept_loop(&listener, &active, &dial_host, target_port));
        inner
            .port_to_session
            .insert(i64::from(port), session_id.to_string());
        inner.sessions.insert(session_id.to_string(), session);
        Ok(Bound {
            listen_host: host,
            listen_port: i64::from(port),
            target_host,
            target_port,
        })
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

/// `net.Listen("tcp", address)`, with Go's address rules and error text: the
/// listener, the IP it reports (`TCPAddr.IP.String()`), and its port. An
/// unspecified or empty host listens dual-stack and reports `::`; a name
/// resolves to its first IPv4 address when it has one.
pub fn listen(address: &str) -> Result<(TcpListener, String, u16), String> {
    let addr_error = |reason: &str| format!("listen tcp: address {address}: {reason}");
    let (host, port) = split_host_port(address).map_err(|reason| addr_error(&reason))?;
    let port: u16 = match port.parse::<i64>() {
        Ok(p) if (0..=65535).contains(&p) => p as u16,
        _ => return Err(format!("listen tcp: address {port}: invalid port")),
    };
    let ip: IpAddr = if host.is_empty() {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else if let Ok(ip) = host.parse::<IpAddr>() {
        ip
    } else {
        let resolved: Vec<SocketAddr> = (host.as_str(), port)
            .to_socket_addrs()
            .map(Iterator::collect)
            .unwrap_or_default();
        match resolved
            .iter()
            .find(|a| a.is_ipv4())
            .or_else(|| resolved.first())
        {
            Some(a) => a.ip(),
            None => return Err(format!("listen tcp: lookup {host}: no such host")),
        }
    };
    let shown = SocketAddr::new(ip, port);
    let bind = if ip.is_unspecified() {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port)
    } else {
        shown
    };
    let listener = TcpListener::bind(bind)
        .or_else(|e| {
            // A host without IPv6 listens on the IPv4 wildcard, as Go does.
            if ip.is_unspecified() && e.raw_os_error() == Some(nix::libc::EAFNOSUPPORT) {
                TcpListener::bind(shown)
            } else {
                Err(e)
            }
        })
        .map_err(|e| format!("listen tcp {shown}: bind: {}", crate::goerr::errno_text(&e)))?;
    let local = listener.local_addr().map_err(|e| e.to_string())?;
    Ok((listener, local.ip().to_string(), local.port()))
}

/// `net.SplitHostPort`.
fn split_host_port(address: &str) -> Result<(String, String), String> {
    let Some(colon) = address.rfind(':') else {
        return Err("missing port in address".into());
    };
    let (host, port) = (&address[..colon], &address[colon + 1..]);
    if let Some(rest) = host.strip_prefix('[') {
        let Some(inner) = rest.strip_suffix(']') else {
            return Err("missing ']' in address".into());
        };
        if inner.contains('[') || inner.contains(']') {
            return Err("unexpected '[' in address".into());
        }
        return Ok((inner.to_string(), port.to_string()));
    }
    if host.contains(':') {
        return Err("too many colons in address".into());
    }
    if host.contains('[') || host.contains(']') {
        return Err("unexpected ']' in address".into());
    }
    Ok((host.to_string(), port.to_string()))
}

fn accept_loop(
    listener: &TcpListener,
    active: &Arc<Mutex<Vec<TcpStream>>>,
    target_host: &str,
    target_port: i64,
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
    target_port: i64,
) -> io::Result<()> {
    // `net.Dial("tcp", JoinHostPort(host, port))`.
    let upstream = match u16::try_from(target_port)
        .ok()
        .and_then(|port| TcpStream::connect((target_host, port)).ok())
    {
        Some(upstream) => upstream,
        None => return Ok(()),
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
        let bound = manager
            .start("a", "127.0.0.1", 0, "127.0.0.1", i64::from(target))
            .unwrap();
        assert_eq!(bound.listen_host, "127.0.0.1");
        let port = bound.listen_port;
        assert_eq!(
            manager
                .start("a", "127.0.0.1", 0, "127.0.0.1", i64::from(target))
                .unwrap_err(),
            "TCP relay session 'a' is already active"
        );
        assert_eq!(
            manager
                .start("b", "127.0.0.1", port, "127.0.0.1", i64::from(target))
                .unwrap_err(),
            format!("TCP relay listen port {port} is already in use by a")
        );
        let mut client = TcpStream::connect(("127.0.0.1", port as u16)).unwrap();
        client.write_all(b"ping").unwrap();
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
        assert!(manager.stop("a"));
        assert!(!manager.stop("a"));
    }

    #[test]
    fn listens_as_go_does() {
        // Go listens on an unspecified host dual-stack and reports `::`.
        let (_l, host, _) = listen("0.0.0.0:0").unwrap();
        assert_eq!(host, "::");
        let (_l, host, _) = listen(":0").unwrap();
        assert_eq!(host, "::");
        let (_l, host, _) = listen("localhost:0").unwrap();
        assert_eq!(host, "127.0.0.1");
        let (_l, host, _) = listen("[::1]:0").unwrap();
        assert_eq!(host, "::1");
        assert_eq!(
            listen("::1:0").unwrap_err(),
            "listen tcp: address ::1:0: too many colons in address"
        );
        assert_eq!(
            listen("[::1:0").unwrap_err(),
            "listen tcp: address [::1:0: missing ']' in address"
        );
        assert_eq!(
            listen("127.0.0.1:70000").unwrap_err(),
            "listen tcp: address 70000: invalid port"
        );
        let (held, _, port) = listen("127.0.0.1:0").unwrap();
        assert_eq!(
            listen(&format!("127.0.0.1:{port}")).unwrap_err(),
            format!("listen tcp 127.0.0.1:{port}: bind: address already in use")
        );
        drop(held);
    }
}
