//! A TCP proxy in front of PostgreSQL that closes a connection at one statement, so a test can
//! fail a connection at a known point of the protocol and not at a point of its timing.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// Where the proxy closes the connection.
#[derive(Clone, Copy)]
pub enum Fault {
    /// At the first message from the client that holds these bytes, which the server never gets.
    Request(&'static [u8]),
    /// At the first message from the server that holds these bytes, which the client never gets.
    /// The server has sent it only after it ran the statement and committed.
    Reply(&'static [u8]),
    /// Like `Request`, but the server gets the message, and the connection stays open until
    /// [`Proxy::release`], so the test can see the server run the statement first.
    HeldRequest(&'static [u8]),
    /// Like `Reply`, but the connection stays open until [`Proxy::release`], so the test can act
    /// after the server committed and before the client learns that the connection failed.
    HeldReply(&'static [u8]),
}

/// What a message does to the armed fault.
enum Hit {
    Missed,
    /// The connection is closed at once, the message not forwarded.
    Closed,
    /// The message is forwarded where the fault says so, and the connection is closed on release.
    Held {
        forward: bool,
    },
}

/// What the proxy and its connections share.
struct Shared {
    armed: Mutex<Option<Fault>>,
    fired: Notify,
    released: Notify,
}

/// A proxy to one PostgreSQL server that closes the connection once, at the armed fault.
pub struct Proxy {
    address: SocketAddr,
    shared: Arc<Shared>,
}

impl Proxy {
    /// Listens on a port of the loopback interface and forwards each connection to `upstream`,
    /// a `host:port`.
    ///
    /// # Panics
    ///
    /// Panics if no loopback port can be bound.
    pub async fn start(upstream: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let shared = Arc::new(Shared {
            armed: Mutex::new(None),
            fired: Notify::new(),
            released: Notify::new(),
        });
        let faults = shared.clone();
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
        Self { address, shared }
    }

    /// `url`, a PostgreSQL URL naming `upstream`, as it names this proxy, without TLS: the proxy
    /// reads the protocol.
    ///
    /// # Panics
    ///
    /// Panics if `url` does not hold `upstream`.
    pub fn url(&self, url: &str, upstream: &str) -> String {
        let proxied = url.replacen(upstream, &self.address.to_string(), 1);
        assert_ne!(proxied, url, "the URL names {upstream}");
        format!("{proxied}?sslmode=disable")
    }

    /// Closes the next connection at `fault`.
    pub fn arm(&self, fault: Fault) {
        *lock(&self.shared.armed) = Some(fault);
    }

    /// Whether the armed fault has closed a connection or holds one.
    pub fn spent(&self) -> bool {
        lock(&self.shared.armed).is_none()
    }

    /// Waits until a held fault holds its connection.
    pub async fn held(&self) {
        self.shared.fired.notified().await;
    }

    /// Closes the connection a held fault holds, now or as soon as it holds one.
    pub fn release(&self) {
        self.shared.released.notify_one();
    }
}

fn lock(armed: &Mutex<Option<Fault>>) -> std::sync::MutexGuard<'_, Option<Fault>> {
    armed.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Copies both ways until either side ends or a fault closes the connection; dropping the halves
/// closes both sockets.
async fn forward(client: TcpStream, server: TcpStream, shared: Arc<Shared>) {
    let (from_client, to_client) = client.into_split();
    let (from_server, to_server) = server.into_split();
    tokio::select! {
        () = pump(from_client, to_server, &shared, true) => {}
        () = pump(from_server, to_client, &shared, false) => {}
    }
}

async fn pump(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, shared: &Shared, request: bool) {
    let mut buffer = [0u8; 16 * 1024];
    while let Ok(read) = from.read(&mut buffer).await {
        if read == 0 {
            return;
        }
        let message = &buffer[..read];
        match fires(&shared.armed, message, request) {
            Hit::Missed => {}
            Hit::Closed => return,
            Hit::Held { forward } => {
                if forward && to.write_all(message).await.is_err() {
                    return;
                }
                shared.fired.notify_one();
                shared.released.notified().await;
                return;
            }
        }
        if to.write_all(message).await.is_err() {
            return;
        }
    }
}

/// What the armed fault does at `message`, a request where `request`; a fault that fires is spent.
fn fires(armed: &Mutex<Option<Fault>>, message: &[u8], request: bool) -> Hit {
    let mut fault = lock(armed);
    let hit = match *fault {
        Some(Fault::Request(bytes)) if request && contains(message, bytes) => Hit::Closed,
        Some(Fault::Reply(bytes)) if !request && contains(message, bytes) => Hit::Closed,
        Some(Fault::HeldRequest(bytes)) if request && contains(message, bytes) => {
            Hit::Held { forward: true }
        }
        Some(Fault::HeldReply(bytes)) if !request && contains(message, bytes) => {
            Hit::Held { forward: false }
        }
        Some(_) | None => Hit::Missed,
    };
    if !matches!(hit, Hit::Missed) {
        *fault = None;
    }
    hit
}

fn contains(message: &[u8], bytes: &[u8]) -> bool {
    message.windows(bytes.len()).any(|window| window == bytes)
}
