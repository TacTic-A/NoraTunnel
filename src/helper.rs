use anyhow::{anyhow, Result};
use async_http_proxy::{http_connect_tokio, http_connect_tokio_with_basic_auth};
use backoff::{backoff::Backoff, Notify};
use socket2::{SockRef, TcpKeepalive};
use std::{future::Future, net::SocketAddr, time::Duration};
use tokio::{
    net::{lookup_host, TcpStream, ToSocketAddrs, UdpSocket},
    sync::broadcast,
};
use tracing::trace;
use url::Url;

use crate::transport::AddrMaybeCached;

// Tokio hesitates to expose this option...So we have to do it on our own :(
// The good news is that using socket2 it can be easily done, without losing portability.
// See https://github.com/tokio-rs/tokio/issues/3082
pub const DEFAULT_TCP_KEEPALIVE_TIME: Duration = Duration::from_secs(10);
pub const DEFAULT_TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(3);
pub const DEFAULT_TCP_KEEPALIVE_RETRIES: u32 = 3;

/// Set TCP keepalive parameters including time, interval, and retry count (probes).
/// On platforms supporting TCP_KEEPCNT (Linux, BSD, Apple), retries are configured.
/// On Windows, time and interval are configured via SIO_KEEPALIVE_VALS.
pub fn try_set_tcp_keepalive_with_probes(
    conn: &TcpStream,
    keepalive_duration: Duration,
    keepalive_interval: Duration,
    retries: u32,
) -> Result<()> {
    let s = SockRef::from(conn);
    #[allow(unused_mut)]
    let mut keepalive = TcpKeepalive::new()
        .with_time(keepalive_duration)
        .with_interval(keepalive_interval);

    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "illumos",
        target_os = "linux",
        target_os = "netbsd",
        target_vendor = "apple",
    ))]
    {
        keepalive = keepalive.with_retries(retries);
    }

    trace!(
        "Set TCP keepalive idle={:?}, interval={:?}, retries={}",
        keepalive_duration,
        keepalive_interval,
        retries
    );

    Ok(s.set_tcp_keepalive(&keepalive)?)
}

/// Backward-compatible wrapper that applies default retry probes (3)
pub fn try_set_tcp_keepalive(
    conn: &TcpStream,
    keepalive_duration: Duration,
    keepalive_interval: Duration,
) -> Result<()> {
    try_set_tcp_keepalive_with_probes(
        conn,
        keepalive_duration,
        keepalive_interval,
        DEFAULT_TCP_KEEPALIVE_RETRIES,
    )
}

/// Configure TCP socket with low-latency TCP_NODELAY and robust keepalive parameters
/// (idle=10s, interval=3s, retries=3) to detect dropped links within 19s.
pub fn tune_tcp_socket(conn: &TcpStream) -> Result<()> {
    conn.set_nodelay(true)?;
    try_set_tcp_keepalive_with_probes(
        conn,
        DEFAULT_TCP_KEEPALIVE_TIME,
        DEFAULT_TCP_KEEPALIVE_INTERVAL,
        DEFAULT_TCP_KEEPALIVE_RETRIES,
    )?;
    Ok(())
}

#[allow(dead_code)]
pub fn feature_not_compile(feature: &str) -> ! {
    panic!(
        "The feature '{}' is not compiled in this binary. Please re-compile NoraTunnel",
        feature
    )
}

pub async fn to_socket_addr<A: ToSocketAddrs>(addr: A) -> Result<SocketAddr> {
    lookup_host(addr)
        .await?
        .next()
        .ok_or_else(|| anyhow!("Failed to lookup the host"))
}

pub fn host_port_pair(s: &str) -> Result<(&str, u16)> {
    let semi = s.rfind(':').expect("missing semicolon");
    Ok((&s[..semi], s[semi + 1..].parse()?))
}

/// Create a UDP socket and connect to `addr`
pub async fn udp_connect<A: ToSocketAddrs>(addr: A) -> Result<UdpSocket> {
    let addr = to_socket_addr(addr).await?;

    let bind_addr = match addr {
        SocketAddr::V4(_) => "0.0.0.0:0",
        SocketAddr::V6(_) => ":::0",
    };

    let s = UdpSocket::bind(bind_addr).await?;
    s.connect(addr).await?;
    Ok(s)
}

/// Create a TcpStream using a proxy
/// e.g. socks5://user:pass@127.0.0.1:1080 http://127.0.0.1:8080
pub async fn tcp_connect_with_proxy(
    addr: &AddrMaybeCached,
    proxy: Option<&Url>,
) -> Result<TcpStream> {
    if let Some(url) = proxy {
        let addr = &addr.addr;
        let mut s = TcpStream::connect((
            url.host_str().expect("proxy url should have host field"),
            url.port().expect("proxy url should have port field"),
        ))
        .await?;

        let auth = if !url.username().is_empty() || url.password().is_some() {
            Some(async_socks5::Auth {
                username: url.username().into(),
                password: url.password().unwrap_or("").into(),
            })
        } else {
            None
        };
        match url.scheme() {
            "socks5" => {
                async_socks5::connect(&mut s, host_port_pair(addr)?, auth).await?;
            }
            "http" => {
                let (host, port) = host_port_pair(addr)?;
                match auth {
                    Some(auth) => {
                        http_connect_tokio_with_basic_auth(
                            &mut s,
                            host,
                            port,
                            &auth.username,
                            &auth.password,
                        )
                        .await?
                    }
                    None => http_connect_tokio(&mut s, host, port).await?,
                }
            }
            _ => panic!("unknown proxy scheme"),
        }
        Ok(s)
    } else {
        Ok(match addr.socket_addr {
            Some(s) => TcpStream::connect(s).await?,
            None => TcpStream::connect(&addr.addr).await?,
        })
    }
}

// Wrapper of retry_notify
pub async fn retry_notify_with_deadline<I, E, Fn, Fut, B, N>(
    backoff: B,
    operation: Fn,
    notify: N,
    deadline: &mut broadcast::Receiver<bool>,
) -> Result<I>
where
    E: std::error::Error + Send + Sync + 'static,
    B: Backoff,
    Fn: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<I, backoff::Error<E>>>,
    N: Notify<E>,
{
    tokio::select! {
        v = backoff::future::retry_notify(backoff, operation, notify) => {
            v.map_err(anyhow::Error::new)
        }
        _ = deadline.recv() => {
            Err(anyhow!("shutdown"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tune_tcp_socket() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client_task = tokio::spawn(async move {
            tokio::net::TcpStream::connect(addr).await.unwrap()
        });
        let (server_sock, _) = listener.accept().await.unwrap();
        let client_sock = client_task.await.unwrap();

        assert!(tune_tcp_socket(&client_sock).is_ok());
        assert!(tune_tcp_socket(&server_sock).is_ok());
    }
}
