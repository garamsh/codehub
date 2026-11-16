//! A TCP proxy in front of PostgreSQL that closes a connection at one statement, so a test can
//! fail a connection at a known point of the protocol and not at a point of its timing.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};

/// Where the proxy closes the connection.
#[derive(Clone, Copy)]
pub enum Fault {
    /// At the first message from the client that holds these bytes, which the server never gets.
    Request(&'static [u8]),
    /// At the first message from the server that holds these bytes, which the client never gets.
    /// The server has sent it only after it ran the statement and committed.
    Reply(&'static [u8]),
}

/// A proxy to one PostgreSQL server that closes the connection once, at the armed fault.
pub struct Proxy {
    address: SocketAddr,
    armed: Arc<Mutex<Option<Fault>>>,
}

impl Proxy {
    /// Listens on a port of the loopback interface and forwards each connection to `upstream`,
    /// a `host:port`.
    pub async fn start(upstream: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let armed = Arc::new(Mutex::new(None));
        let faults = armed.clone();
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let (upstream, faults) = (upstream.clone(), faults.clone());
                tokio::spawn(async move {
                    if let Ok(server) = TcpStream::connect(upstream).await {
                        forward(client, server, faults).await;
                    }
                });
            }
        });
        Self { address, armed }
    }

    /// `url`, a PostgreSQL URL naming `upstream`, as it names this proxy, without TLS: the proxy
    /// reads the protocol.
    pub fn url(&self, url: &str, upstream: &str) -> String {
        let proxied = url.replacen(upstream, &self.address.to_string(), 1);
        assert_ne!(proxied, url, "the URL names {upstream}");
        format!("{proxied}?sslmode=disable")
    }

    /// Closes the next connection at `fault`.
    pub fn arm(&self, fault: Fault) {
        *lock(&self.armed) = Some(fault);
    }

    /// Whether the armed fault has closed a connection.
    pub fn spent(&self) -> bool {
        lock(&self.armed).is_none()
    }
}

fn lock(armed: &Mutex<Option<Fault>>) -> std::sync::MutexGuard<'_, Option<Fault>> {
    armed.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Copies both ways until either side ends or a fault closes the connection; dropping the halves
/// closes both sockets.
async fn forward(client: TcpStream, server: TcpStream, armed: Arc<Mutex<Option<Fault>>>) {
    let (from_client, to_client) = client.into_split();
    let (from_server, to_server) = server.into_split();
    tokio::select! {
        () = pump(from_client, to_server, &armed, true) => {}
        () = pump(from_server, to_client, &armed, false) => {}
    }
}

async fn pump(
    mut from: OwnedReadHalf,
    mut to: OwnedWriteHalf,
    armed: &Mutex<Option<Fault>>,
    request: bool,
) {
    let mut buffer = [0u8; 16 * 1024];
    while let Ok(read) = from.read(&mut buffer).await {
        if read == 0 {
            return;
        }
        let message = &buffer[..read];
        if fires(armed, message, request) {
            return;
        }
        if to.write_all(message).await.is_err() {
            return;
        }
    }
}

/// Whether the armed fault is at `message`, a request where `request`; a fault that fires is spent.
fn fires(armed: &Mutex<Option<Fault>>, message: &[u8], request: bool) -> bool {
    let mut fault = lock(armed);
    let hit = match *fault {
        Some(Fault::Request(bytes)) => request && contains(message, bytes),
        Some(Fault::Reply(bytes)) => !request && contains(message, bytes),
        None => false,
    };
    if hit {
        *fault = None;
    }
    hit
}

fn contains(message: &[u8], bytes: &[u8]) -> bool {
    message.windows(bytes.len()).any(|window| window == bytes)
}
