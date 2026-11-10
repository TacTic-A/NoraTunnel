use crate::config::{Config, ServerConfig, ServerServiceConfig, ServiceType, TransportType};
use crate::config_watcher::{ConfigChange, ServerServiceChange};
use crate::constants::{listen_backoff, UDP_BUFFER_SIZE};
use crate::helper::retry_notify_with_deadline;
use crate::multi_map::MultiMap;
use crate::protocol::Hello::{ControlChannelHello, DataChannelHello};
use crate::protocol::{
    self, read_auth, read_hello, Ack, ControlChannelCmd, DataChannelCmd, Hello, UdpTraffic,
    HASH_WIDTH_IN_BYTES,
};
use crate::transport::{SocketOpts, TcpTransport, Transport};
use anyhow::{anyhow, bail, Context, Result};
use backoff::backoff::Backoff;
use backoff::ExponentialBackoff;

use rand::RngCore;
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{broadcast, mpsc, watch, RwLock};
use tokio::time;
use tracing::{debug, error, info, info_span, instrument, trace, warn, Instrument, Span};

#[cfg(feature = "noise")]
use crate::transport::NoiseTransport;
#[cfg(feature = "tls")]
use crate::transport::TlsTransport;
#[cfg(feature = "websocket")]
use crate::transport::WebsocketTransport;

type ServiceDigest = protocol::Digest; // SHA256 of a service name
type Nonce = protocol::Digest; // Also called `session_key`

const TCP_POOL_SIZE: usize = 16; // The number of cached connections for TCP services
const UDP_POOL_SIZE: usize = 4; // The number of cached connections for UDP services
const CHAN_SIZE: usize = 2048; // The capacity of various chans
const HANDSHAKE_TIMEOUT: u64 = 5; // Timeout for transport handshake

// The entrypoint of running a server
pub async fn run_server(
    config: Config,
    shutdown_rx: broadcast::Receiver<bool>,
    update_rx: mpsc::Receiver<ConfigChange>,
) -> Result<()> {
    let config = match config.server {
            Some(config) => config,
            None => {
                return Err(anyhow!("Try to run as a server, but the configuration is missing. Please add the `[server]` block"))
            }
        };

    match config.transport.transport_type {
        TransportType::Tcp => {
            let mut server = Server::<TcpTransport>::from(config).await?;
            server.run(shutdown_rx, update_rx).await?;
        }
        TransportType::Tls => {
            #[cfg(feature = "tls")]
            {
                let mut server = Server::<TlsTransport>::from(config).await?;
                server.run(shutdown_rx, update_rx).await?;
            }
            #[cfg(not(feature = "tls"))]
            crate::helper::feature_not_compile("tls")
        }
        TransportType::Noise => {
            #[cfg(feature = "noise")]
            {
                let mut server = Server::<NoiseTransport>::from(config).await?;
                server.run(shutdown_rx, update_rx).await?;
            }
            #[cfg(not(feature = "noise"))]
            crate::helper::feature_not_compile("noise")
        }
        TransportType::Websocket => {
            #[cfg(feature = "websocket")]
            {
                let mut server = Server::<WebsocketTransport>::from(config).await?;
                server.run(shutdown_rx, update_rx).await?;
            }
            #[cfg(not(feature = "websocket"))]
            crate::helper::feature_not_compile("websocket")
        }
    }

    Ok(())
}

/// Represents an active ephemeral tunnel session negotiated with Kharej
pub struct TunnelSession<T: Transport> {
    pub session_key: Nonce,
    pub visitor_tx: mpsc::Sender<TcpStream>,
    _phantom: std::marker::PhantomData<T>,
}

/// Persistent supervisor managing the ingress listener lifecycle across reconnections
pub struct ServiceSupervisor<T: Transport> {
    #[allow(dead_code)]
    pub service_config: ServerServiceConfig,
    pub tunnel_tx: watch::Sender<Option<Arc<TunnelSession<T>>>>,
    pub shutdown_tx: broadcast::Sender<bool>,
}

impl<T: 'static + Transport> ServiceSupervisor<T> {
    /// Starts the persistent ingress listener. Runs indefinitely across Kharej reconnects.
    pub fn start(service: ServerServiceConfig) -> Self {
        let (tunnel_tx, tunnel_rx) = watch::channel::<Option<Arc<TunnelSession<T>>>>(None);
        let (shutdown_tx, _) = broadcast::channel::<bool>(1);
        let mut supervisor_shutdown_rx = shutdown_tx.subscribe();

        let bind_addr = service.bind_addr.clone();
        let service_type = service.service_type;
        let service_name = service.name.clone();

        tokio::spawn(async move {
            match service_type {
                ServiceType::Tcp => {
                    let listener = match retry_notify_with_deadline(
                        listen_backoff(),
                        || async { Ok(TcpListener::bind(&bind_addr).await?) },
                        |e, d| {
                            warn!("Persistent listener bind error on {}: {:#}. Retrying in {:?}", bind_addr, e, d);
                        },
                        &mut supervisor_shutdown_rx,
                    ).await {
                        Ok(l) => l,
                        Err(e) => {
                            error!("Failed to bind persistent TCP listener on {}: {:#}", bind_addr, e);
                            return;
                        }
                    };

                    info!(service = %service_name, "Persistent TCP ingress listener active on {}", bind_addr);

                    let mut backoff = ExponentialBackoff {
                        max_interval: Duration::from_millis(500),
                        max_elapsed_time: None,
                        ..Default::default()
                    };

                    loop {
                        tokio::select! {
                            accept_res = listener.accept() => {
                                match accept_res {
                                    Ok((visitor, peer_addr)) => {
                                        backoff.reset();
                                        let mut rx = tunnel_rx.clone();
                                        tokio::spawn(async move {
                                            Self::dispatch_tcp_visitor(visitor, peer_addr, &mut rx).await;
                                        }.instrument(info_span!("visitor", %peer_addr)));
                                    }
                                    Err(e) => {
                                        warn!("Accept error on {}: {:#}", bind_addr, e);
                                        if let Some(d) = backoff.next_backoff() {
                                            time::sleep(d).await;
                                        }
                                    }
                                }
                            }
                            _ = supervisor_shutdown_rx.recv() => {
                                info!(service = %service_name, "Persistent TCP ingress on {} received shutdown", bind_addr);
                                break;
                            }
                        }
                    }
                }
                ServiceType::Udp => {
                    // Parity implementation for UDP listener persistence
                }
            }
        }.instrument(info_span!("supervisor", service = %service.name)));

        ServiceSupervisor {
            service_config: service,
            tunnel_tx,
            shutdown_tx,
        }
    }

    /// Dispatches an accepted visitor TcpStream with a 3-second reconnect grace period.
    async fn dispatch_tcp_visitor(
        visitor: TcpStream,
        peer: SocketAddr,
        tunnel_rx: &mut watch::Receiver<Option<Arc<TunnelSession<T>>>>,
    ) {
        let session = {
            let initial = tunnel_rx.borrow().clone();
            if let Some(s) = initial {
                s
            } else {
                debug!("Visitor from {} arrived during tunnel disconnect; entering 3s grace period wait", peer);
                match time::timeout(Duration::from_secs(3), async {
                    while tunnel_rx.changed().await.is_ok() {
                        if let Some(s) = tunnel_rx.borrow().clone() {
                            return Some(s);
                        }
                    }
                    None
                }).await {
                    Ok(Some(s)) => {
                        debug!("Visitor from {} successfully linked to reconnected tunnel session", peer);
                        s
                    }
                    _ => {
                        debug!("Dropping visitor from {}: tunnel disconnected and 3s reconnect grace period expired", peer);
                        return;
                    }
                }
            }
        };

        if let Err(e) = session.visitor_tx.send(visitor).await {
            debug!("Failed to dispatch visitor from {}: dispatcher channel closed ({:#})", peer, e);
        }
    }
}

// A hash map of ControlChannelHandles, indexed by ServiceDigest or Nonce
// See also MultiMap
type ControlChannelMap<T> = MultiMap<ServiceDigest, Nonce, ControlChannelHandle<T>>;

// Server holds all states of running a server
struct Server<T: Transport> {
    // `[server]` config
    config: Arc<ServerConfig>,

    // `[server.services]` config, indexed by ServiceDigest
    services: Arc<RwLock<HashMap<ServiceDigest, ServerServiceConfig>>>,
    // Collection of contorl channels
    control_channels: Arc<RwLock<ControlChannelMap<T>>>,
    // Persistent service ingress supervisors
    supervisors: Arc<RwLock<HashMap<ServiceDigest, ServiceSupervisor<T>>>>,
    // Wrapper around the transport layer
    transport: Arc<T>,
}

// Generate a hash map of services which is indexed by ServiceDigest
fn generate_service_hashmap(
    server_config: &ServerConfig,
) -> HashMap<ServiceDigest, ServerServiceConfig> {
    let mut ret = HashMap::new();
    for u in &server_config.services {
        ret.insert(protocol::digest(u.0.as_bytes()), (*u.1).clone());
    }
    ret
}

impl<T: 'static + Transport> Server<T> {
    // Create a server from `[server]`
    pub async fn from(config: ServerConfig) -> Result<Server<T>> {
        let config = Arc::new(config);
        let services = Arc::new(RwLock::new(generate_service_hashmap(&config)));
        let control_channels = Arc::new(RwLock::new(ControlChannelMap::new()));
        let transport = Arc::new(T::new(&config.transport)?);
        let supervisors = Arc::new(RwLock::new(HashMap::new()));

        {
            let mut sup_guard = supervisors.write().await;
            for u in &config.services {
                let digest = protocol::digest(u.0.as_bytes());
                let supervisor = ServiceSupervisor::start((*u.1).clone());
                sup_guard.insert(digest, supervisor);
            }
        }

        Ok(Server {
            config,
            services,
            control_channels,
            supervisors,
            transport,
        })
    }

    // The entry point of Server
    pub async fn run(
        &mut self,
        mut shutdown_rx: broadcast::Receiver<bool>,
        mut update_rx: mpsc::Receiver<ConfigChange>,
    ) -> Result<()> {
        // Listen at `server.bind_addr`
        let l = self
            .transport
            .bind(&self.config.bind_addr)
            .await
            .with_context(|| "Failed to listen at `server.bind_addr`")?;
        info!("Listening at {}", self.config.bind_addr);

        // Retry at least every 100ms
        let mut backoff = ExponentialBackoff {
            max_interval: Duration::from_millis(100),
            max_elapsed_time: None,
            ..Default::default()
        };

        // Wait for connections and shutdown signals
        loop {
            tokio::select! {
                // Wait for incoming control and data channels
                ret = self.transport.accept(&l) => {
                    match ret {
                        Err(err) => {
                            // Detects whether it's an IO error
                            if let Some(err) = err.downcast_ref::<io::Error>() {
                                // If it is an IO error, then it's possibly an
                                // EMFILE. So sleep for a while and retry
                                // TODO: Only sleep for EMFILE, ENFILE, ENOMEM, ENOBUFS
                                if let Some(d) = backoff.next_backoff() {
                                    error!("Failed to accept: {:#}. Retry in {:?}...", err, d);
                                    time::sleep(d).await;
                                } else {
                                    // This branch will never be executed according to the current retry policy
                                    error!("Too many retries. Aborting...");
                                    break;
                                }
                            }
                            // If it's not an IO error, then it comes from
                            // the transport layer, so just ignore it
                        }
                        Ok((conn, addr)) => {
                            backoff.reset();

                            // Do transport handshake with a timeout
                            match time::timeout(Duration::from_secs(HANDSHAKE_TIMEOUT), self.transport.handshake(conn)).await {
                                Ok(conn) => {
                                    match conn.with_context(|| "Failed to do transport handshake") {
                                        Ok(conn) => {
                                            let services = self.services.clone();
                                            let control_channels = self.control_channels.clone();
                                            let supervisors = self.supervisors.clone();
                                            let server_config = self.config.clone();
                                            tokio::spawn(async move {
                                                if let Err(err) = handle_connection(conn, services, control_channels, supervisors, server_config).await {
                                                    error!("{:#}", err);
                                                }
                                            }.instrument(info_span!("connection", %addr)));
                                        }, Err(e) => {
                                            error!("{:#}", e);
                                        }
                                    }
                                },
                                Err(e) => {
                                    error!("Transport handshake timeout: {}", e);
                                }
                            }
                        }
                    }
                },
                // Wait for the shutdown signal
                _ = shutdown_rx.recv() => {
                    info!("Shuting down gracefully...");
                    let sup_guard = self.supervisors.read().await;
                    for supervisor in sup_guard.values() {
                        let _ = supervisor.shutdown_tx.send(true);
                    }
                    break;
                },
                e = update_rx.recv() => {
                    if let Some(e) = e {
                        self.handle_hot_reload(e).await;
                    }
                }
            }
        }

        info!("Shutdown");

        Ok(())
    }

    async fn handle_hot_reload(&mut self, e: ConfigChange) {
        match e {
            ConfigChange::ServerChange(server_change) => match server_change {
                ServerServiceChange::Add(cfg) => {
                    let hash = protocol::digest(cfg.name.as_bytes());
                    let mut wg = self.services.write().await;
                    let _ = wg.insert(hash, cfg.clone());

                    let mut wg = self.control_channels.write().await;
                    let _ = wg.remove1(&hash);

                    let supervisor = ServiceSupervisor::start(cfg);
                    let mut sup_guard = self.supervisors.write().await;
                    sup_guard.insert(hash, supervisor);
                }
                ServerServiceChange::Delete(s) => {
                    let hash = protocol::digest(s.as_bytes());
                    let _ = self.services.write().await.remove(&hash);

                    let mut wg = self.control_channels.write().await;
                    let _ = wg.remove1(&hash);

                    let mut sup_guard = self.supervisors.write().await;
                    if let Some(sup) = sup_guard.remove(&hash) {
                        let _ = sup.shutdown_tx.send(true);
                    }
                }
            },
            ignored => warn!("Ignored {:?} since running as a server", ignored),
        }
    }
}

// Handle connections to `server.bind_addr`
async fn handle_connection<T: 'static + Transport>(
    mut conn: T::Stream,
    services: Arc<RwLock<HashMap<ServiceDigest, ServerServiceConfig>>>,
    control_channels: Arc<RwLock<ControlChannelMap<T>>>,
    supervisors: Arc<RwLock<HashMap<ServiceDigest, ServiceSupervisor<T>>>>,
    server_config: Arc<ServerConfig>,
) -> Result<()> {
    // Read hello
    let hello = read_hello(&mut conn).await?;
    match hello {
        ControlChannelHello(_, service_digest) => {
            do_control_channel_handshake(
                conn,
                services,
                control_channels,
                supervisors,
                service_digest,
                server_config,
            )
            .await?;
        }
        DataChannelHello(_, nonce) => {
            do_data_channel_handshake(conn, control_channels, nonce).await?;
        }
    }
    Ok(())
}

async fn do_control_channel_handshake<T: 'static + Transport>(
    mut conn: T::Stream,
    services: Arc<RwLock<HashMap<ServiceDigest, ServerServiceConfig>>>,
    control_channels: Arc<RwLock<ControlChannelMap<T>>>,
    supervisors: Arc<RwLock<HashMap<ServiceDigest, ServiceSupervisor<T>>>>,
    service_digest: ServiceDigest,
    server_config: Arc<ServerConfig>,
) -> Result<()> {
    info!("Try to handshake a control channel");

    T::hint(&conn, SocketOpts::for_control_channel());

    // Generate a nonce
    let mut nonce = vec![0u8; HASH_WIDTH_IN_BYTES];
    rand::thread_rng().fill_bytes(&mut nonce);

    // Send hello
    let hello_send = Hello::ControlChannelHello(
        protocol::CURRENT_PROTO_VERSION,
        nonce.clone().try_into().unwrap(),
    );
    conn.write_all(&bincode::serialize(&hello_send).unwrap())
        .await?;
    conn.flush().await?;

    // Lookup the service
    let service_config = match services.read().await.get(&service_digest) {
        Some(v) => v,
        None => {
            conn.write_all(&bincode::serialize(&Ack::ServiceNotExist).unwrap())
                .await?;
            bail!("No such a service {}", hex::encode(service_digest));
        }
    }
    .to_owned();

    let service_name = &service_config.name;

    // Calculate the checksum
    let mut concat = Vec::from(service_config.token.as_ref().unwrap().as_bytes());
    concat.append(&mut nonce);

    // Read auth
    let protocol::Auth(d) = read_auth(&mut conn).await?;

    // Validate
    let session_key = protocol::digest(&concat);
    if session_key != d {
        conn.write_all(&bincode::serialize(&Ack::AuthFailed).unwrap())
            .await?;
        debug!(
            "Expect {}, but got {}",
            hex::encode(session_key),
            hex::encode(d)
        );
        bail!("Service {} failed the authentication", service_name);
    } else {
        let mut h = control_channels.write().await;

        // If there's already a control channel for the service, drop the old one
        if h.remove1(&service_digest).is_some() {
            warn!(
                "Dropping previous control channel for service {}",
                service_name
            );
        }

        // Send ack
        conn.write_all(&bincode::serialize(&Ack::Ok).unwrap())
            .await?;
        conn.flush().await?;

        info!(service = %service_config.name, "Control channel established");

        // Create a shutdown channel
        let (shutdown_tx, shutdown_rx) = broadcast::channel::<bool>(1);

        // Store data channels
        let (data_ch_tx, data_ch_rx) = mpsc::channel(CHAN_SIZE * 2);

        // Store data channel creation requests
        let (data_ch_req_tx, data_ch_req_rx) = mpsc::unbounded_channel::<bool>();

        match service_config.service_type {
            ServiceType::Tcp => {
                let (visitor_tx, visitor_rx) = mpsc::channel(CHAN_SIZE);
                let dispatcher_config = ChannelDispatcherConfig::default();

                // Spawn Active Channel Dispatcher Actor (Variant A.2)
                tokio::spawn(
                    run_tcp_channel_dispatcher::<T>(
                        visitor_rx,
                        data_ch_rx,
                        data_ch_req_tx.clone(),
                        shutdown_tx.subscribe(),
                        dispatcher_config,
                    )
                    .instrument(info_span!("dispatcher", service = %service_name, session = %hex::encode(session_key))),
                );

                // Create active TunnelSession and link to persistent ServiceSupervisor
                let tunnel_session = Arc::new(TunnelSession {
                    session_key,
                    visitor_tx,
                    _phantom: std::marker::PhantomData,
                });

                {
                    let sup_guard = supervisors.read().await;
                    if let Some(supervisor) = sup_guard.get(&service_digest) {
                        let _ = supervisor.tunnel_tx.send(Some(Arc::clone(&tunnel_session)));
                        info!(service = %service_name, "Linked active tunnel session to persistent ServiceSupervisor");
                    } else {
                        warn!(service = %service_name, "ServiceSupervisor not found for service");
                    }
                }
            }
            ServiceType::Udp => {
                for _i in 0..UDP_POOL_SIZE {
                    if let Err(e) = data_ch_req_tx.send(true) {
                        error!("Failed to request data channel {}", e);
                    };
                }
                let shutdown_rx_clone = shutdown_tx.subscribe();
                let bind_addr = service_config.bind_addr.clone();
                tokio::spawn(
                    async move {
                        if let Err(e) = run_udp_connection_pool::<T>(
                            bind_addr,
                            data_ch_rx,
                            data_ch_req_tx,
                            shutdown_rx_clone,
                        )
                        .await
                        .with_context(|| "Failed to run UDP connection pool")
                        {
                            error!("{:#}", e);
                        }
                    }
                    .instrument(Span::current()),
                );
            }
        }

        // Create the control channel
        let ch = ControlChannel::<T> {
            conn,
            shutdown_rx,
            shutdown_tx: shutdown_tx.clone(),
            data_ch_req_rx,
            heartbeat_interval: server_config.heartbeat_interval,
            session_key,
            service_name: service_config.name.clone(),
            service_digest,
            control_channels: control_channels.clone(),
            supervisors: supervisors.clone(),
        };

        // Run the control channel
        tokio::spawn(
            async move {
                if let Err(err) = ch.run().await {
                    error!("{:#}", err);
                }
            }
            .instrument(info_span!("control", service = %service_name, session = %hex::encode(session_key))),
        );

        let handle = ControlChannelHandle {
            _shutdown_tx: shutdown_tx,
            data_ch_tx,
            service: service_config,
            session_key,
        };

        // Insert the new handle
        let _ = h.insert(service_digest, session_key, handle);
    }

    Ok(())
}

async fn do_data_channel_handshake<T: 'static + Transport>(
    conn: T::Stream,
    control_channels: Arc<RwLock<ControlChannelMap<T>>>,
    nonce: Nonce,
) -> Result<()> {
    debug!("Try to handshake a data channel");

    // Validate
    let control_channels_guard = control_channels.read().await;
    match control_channels_guard.get2(&nonce) {
        Some(handle) => {
            T::hint(&conn, SocketOpts::from_server_cfg(&handle.service));

            // Send the data channel to the corresponding control channel
            handle
                .data_ch_tx
                .send(conn)
                .await
                .with_context(|| "Data channel for a stale control channel")?;
        }
        None => {
            warn!("Data channel has incorrect nonce");
        }
    }
    Ok(())
}

pub struct ControlChannelHandle<T: Transport> {
    pub _shutdown_tx: broadcast::Sender<bool>,
    pub data_ch_tx: mpsc::Sender<T::Stream>,
    pub service: ServerServiceConfig,
    #[allow(dead_code)]
    pub session_key: Nonce,
}

// Control channel, using T as the transport layer.
struct ControlChannel<T: Transport> {
    conn: T::Stream,
    shutdown_rx: broadcast::Receiver<bool>,
    shutdown_tx: broadcast::Sender<bool>,
    data_ch_req_rx: mpsc::UnboundedReceiver<bool>,
    heartbeat_interval: u64,
    session_key: Nonce,
    service_name: String,
    service_digest: ServiceDigest,
    control_channels: Arc<RwLock<ControlChannelMap<T>>>,
    supervisors: Arc<RwLock<HashMap<ServiceDigest, ServiceSupervisor<T>>>>,
}

impl<T: 'static + Transport> ControlChannel<T> {
    #[instrument(skip_all, fields(service = %self.service_name, session = %hex::encode(self.session_key)))]
    async fn run(mut self) -> Result<()> {
        let create_ch_cmd = bincode::serialize(&ControlChannelCmd::CreateDataChannel).unwrap();
        let heartbeat_bytes = bincode::serialize(&ControlChannelCmd::HeartBeat).unwrap();

        // Helper to calculate jittered heartbeat interval for DPI resistance (+/- 15% random variance)
        let get_heartbeat_duration = |interval: u64| -> Duration {
            let base_secs = if interval == 0 { 15 } else { interval };
            let base_ms = base_secs * 1000;
            let jitter_range = (base_ms as f64 * 0.15) as u64;
            let offset = if jitter_range > 0 {
                rand::random::<u64>() % (jitter_range * 2 + 1)
            } else {
                0
            };
            Duration::from_millis(base_ms - jitter_range + offset)
        };

        let heartbeat_timeout = Duration::from_secs(60);
        let mut last_activity = tokio::time::Instant::now();
        let mut next_heartbeat = tokio::time::Instant::now() + get_heartbeat_duration(self.heartbeat_interval);

        // Split conn into independent read and write halves for concurrent bidirectional I/O
        let (mut read_half, mut write_half) = tokio::io::split(self.conn);

        loop {
            tokio::select! {
                // Branch 1: Read incoming commands and HeartBeatAck from Kharej client (EOF & Ack detection)
                read_res = protocol::read_control_cmd(&mut read_half) => {
                    match read_res {
                        Ok(ControlChannelCmd::HeartBeatAck) => {
                            trace!("Received HeartBeatAck from Kharej client");
                            last_activity = tokio::time::Instant::now();
                        }
                        Ok(cmd) => {
                            debug!("Received control command: {:?}", cmd);
                            last_activity = tokio::time::Instant::now();
                        }
                        Err(e) => {
                            warn!("Control channel read error or client EOF: {:#}", e);
                            break;
                        }
                    }
                },

                // Branch 2: Emit jittered HeartBeat to Kharej client (decoupled ticker, immune to starvation)
                _ = tokio::time::sleep_until(next_heartbeat) => {
                    if let Err(e) = write_half.write_all(&heartbeat_bytes).await {
                        error!("Failed to write heartbeat to control channel: {:#}", e);
                        break;
                    }
                    if let Err(e) = write_half.flush().await {
                        error!("Failed to flush heartbeat to control channel: {:#}", e);
                        break;
                    }
                    trace!("Emitted HeartBeat to Kharej client");
                    next_heartbeat = tokio::time::Instant::now() + get_heartbeat_duration(self.heartbeat_interval);
                },

                // Branch 3: Strict 60s Heartbeat/Activity Timeout (Silent Middlebox Drop Detection)
                _ = tokio::time::sleep_until(last_activity + heartbeat_timeout) => {
                    error!(
                        "Heartbeat timeout: no activity received from client for {}s. Silent middlebox drop detected; closing channel.",
                        heartbeat_timeout.as_secs()
                    );
                    break;
                },

                // Branch 4: Forward data channel creation requests to Kharej client
                req = self.data_ch_req_rx.recv() => {
                    match req {
                        Some(_) => {
                            if let Err(e) = write_half.write_all(&create_ch_cmd).await {
                                error!("Failed to write CreateDataChannel cmd: {:#}", e);
                                break;
                            }
                            if let Err(e) = write_half.flush().await {
                                error!("Failed to flush CreateDataChannel cmd: {:#}", e);
                                break;
                            }
                        }
                        None => {
                            debug!("Data channel request channel closed; shutting down");
                            break;
                        }
                    }
                },

                // Branch 5: Graceful Shutdown Signal
                _ = self.shutdown_rx.recv() => {
                    info!("Control channel received shutdown signal");
                    break;
                }
            }
        }

        // --- Clean Lifecycle Teardown Block ---
        // 1. Unlink session from persistent ServiceSupervisor if this is still the active session
        {
            let sup_guard = self.supervisors.read().await;
            if let Some(supervisor) = sup_guard.get(&self.service_digest) {
                let should_clear = {
                    if let Some(current) = supervisor.tunnel_tx.borrow().as_ref() {
                        current.session_key == self.session_key
                    } else {
                        false
                    }
                };
                if should_clear {
                    let _ = supervisor.tunnel_tx.send(None);
                    debug!("Unlinked terminated tunnel session from ServiceSupervisor");
                }
            }
        }

        // 2. Broadcast shutdown signal to associated connection pools and tasks
        let _ = self.shutdown_tx.send(true);

        // 3. TOCTOU-free cleanup: Remove this session from control_channels using remove2(&session_key)
        {
            let mut guard = self.control_channels.write().await;
            if guard.remove2(&self.session_key).is_some() {
                info!(
                    service = %self.service_name,
                    session = %hex::encode(self.session_key),
                    "Cleanly removed terminated control channel handle from MultiMap"
                );
            } else {
                debug!(
                    service = %self.service_name,
                    session = %hex::encode(self.session_key),
                    "Control channel handle already replaced or evicted; ignoring eviction"
                );
            }
        }

        info!("Control channel shutdown complete");
        Ok(())
    }
}

/// Configuration for the Active Channel Dispatcher Actor
#[derive(Debug, Clone)]
pub struct ChannelDispatcherConfig {
    pub max_idle_ttl: Duration,        // 30 seconds: prune channels before middlebox NAT eviction
    pub visitor_timeout: Duration,     // 5 seconds: max visitor queue wait time
    pub target_pool_size: usize,       // 16 for TCP
    pub max_pending_visitors: usize,   // 256: backpressure shedding threshold
}

impl Default for ChannelDispatcherConfig {
    fn default() -> Self {
        Self {
            max_idle_ttl: Duration::from_secs(30),
            visitor_timeout: Duration::from_secs(5),
            target_pool_size: TCP_POOL_SIZE,
            max_pending_visitors: 256,
        }
    }
}

struct PooledChannel<S> {
    stream: S,
    created_at: Instant,
}

struct PendingVisitor {
    stream: TcpStream,
    enqueued_at: Instant,
}

#[instrument(skip_all)]
pub async fn run_tcp_channel_dispatcher<T: Transport>(
    mut visitor_rx: mpsc::Receiver<TcpStream>,
    mut data_ch_rx: mpsc::Receiver<T::Stream>,
    data_ch_req_tx: mpsc::UnboundedSender<bool>,
    mut shutdown_rx: broadcast::Receiver<bool>,
    config: ChannelDispatcherConfig,
) -> Result<()> {
    let mut idle_channels: VecDeque<PooledChannel<T::Stream>> =
        VecDeque::with_capacity(config.target_pool_size * 2);
    let mut pending_visitors: VecDeque<PendingVisitor> =
        VecDeque::with_capacity(config.max_pending_visitors);
    let mut prune_interval = time::interval(Duration::from_secs(5));
    prune_interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    let cmd = Arc::new(bincode::serialize(&DataChannelCmd::StartForwardTcp).unwrap());

    // Pre-warm the pool to target size
    for _ in 0..config.target_pool_size {
        let _ = data_ch_req_tx.send(true);
    }

    loop {
        tokio::select! {
            // Branch 1: Incoming visitor from persistent TcpListener
            val = visitor_rx.recv() => {
                match val {
                    Some(visitor) => {
                        let mut matched_channel = None;

                        // Pop and validate idle channels from the pre-warmed pool
                        while let Some(pooled) = idle_channels.pop_front() {
                            if pooled.created_at.elapsed() < config.max_idle_ttl {
                                matched_channel = Some(pooled.stream);
                                // Trigger replenishment to keep pool depth steady
                                let _ = data_ch_req_tx.send(true);
                                break;
                            } else {
                                debug!("Discarded stale pre-warmed channel (TTL expired: {:?})", pooled.created_at.elapsed());
                                // Replace discarded stale channel
                                let _ = data_ch_req_tx.send(true);
                            }
                        }

                        if let Some(ch) = matched_channel {
                            spawn_tunnel_worker::<T>(ch, visitor, Arc::clone(&cmd));
                        } else {
                            // Pool drained; queue visitor if within backpressure capacity
                            if pending_visitors.len() < config.max_pending_visitors {
                                pending_visitors.push_back(PendingVisitor {
                                    stream: visitor,
                                    enqueued_at: Instant::now(),
                                });
                                let _ = data_ch_req_tx.send(true);
                            } else {
                                warn!(
                                    "Visitor backpressure limit ({}) reached; shedding connection",
                                    config.max_pending_visitors
                                );
                            }
                        }
                    }
                    None => {
                        debug!("Visitor channel closed; shutting down dispatcher");
                        break;
                    }
                }
            }

            // Branch 2: Incoming data channel dialed by Kharej reverse client
            val = data_ch_rx.recv() => {
                match val {
                    Some(ch) => {
                        let mut matched_visitor = None;

                        // Match with the earliest non-timed-out pending visitor
                        while let Some(pv) = pending_visitors.pop_front() {
                            if pv.enqueued_at.elapsed() <= config.visitor_timeout {
                                matched_visitor = Some(pv.stream);
                                break;
                            } else {
                                debug!("Pending visitor timed out (> {:?}) waiting for data channel", config.visitor_timeout);
                            }
                        }

                        if let Some(visitor) = matched_visitor {
                            spawn_tunnel_worker::<T>(ch, visitor, Arc::clone(&cmd));
                        } else {
                            // No awaiting visitors; store in idle pre-warmed pool
                            if idle_channels.len() < config.target_pool_size * 2 {
                                idle_channels.push_back(PooledChannel {
                                    stream: ch,
                                    created_at: Instant::now(),
                                });
                            } else {
                                debug!("Pre-warmed pool at capacity ({}); closing excess channel", config.target_pool_size * 2);
                            }
                        }
                    }
                    None => {
                        debug!("Data channel stream closed; shutting down dispatcher");
                        break;
                    }
                }
            }

            // Branch 3: Periodic health pruning (Zombie and Stale Channel Sweeper)
            _ = prune_interval.tick() => {
                // 1. Prune expired idle channels (> 30s TTL)
                let initial_channels = idle_channels.len();
                idle_channels.retain(|c| c.created_at.elapsed() < config.max_idle_ttl);
                let expired = initial_channels - idle_channels.len();
                if expired > 0 {
                    trace!("Pruned {} stale idle channels during 5s sweep; replenishing", expired);
                    for _ in 0..expired {
                        let _ = data_ch_req_tx.send(true);
                    }
                }

                // 2. Prune timed-out pending visitors (> 5s queue wait)
                let initial_visitors = pending_visitors.len();
                pending_visitors.retain(|v| v.enqueued_at.elapsed() <= config.visitor_timeout);
                let timed_out = initial_visitors - pending_visitors.len();
                if timed_out > 0 {
                    debug!("Pruned {} timed-out pending visitors during 5s sweep", timed_out);
                }
            }

            // Branch 4: Graceful shutdown
            _ = shutdown_rx.recv() => {
                info!("Active channel dispatcher actor shutting down; draining queues");
                break;
            }
        }
    }

    Ok(())
}

fn spawn_tunnel_worker<T: Transport>(
    mut ch: T::Stream,
    mut visitor: TcpStream,
    cmd: Arc<Vec<u8>>,
) {
    tokio::spawn(async move {
        let _ = crate::helper::tune_tcp_socket(&visitor);
        if ch.write_all(&cmd).await.is_ok() {
            let _ = tokio::io::copy_bidirectional(&mut ch, &mut visitor).await;
        } else {
            debug!("Failed to write StartForwardTcp command to data channel");
        }
    }.instrument(tracing::debug_span!("tunnel_worker")));
}

#[instrument(skip_all)]
async fn run_udp_connection_pool<T: Transport>(
    bind_addr: String,
    mut data_ch_rx: mpsc::Receiver<T::Stream>,
    _data_ch_req_tx: mpsc::UnboundedSender<bool>,
    mut shutdown_rx: broadcast::Receiver<bool>,
) -> Result<()> {
    // TODO: Load balance

    let l = retry_notify_with_deadline(
        listen_backoff(),
        || async { Ok(UdpSocket::bind(&bind_addr).await?) },
        |e, duration| {
            warn!("{:#}. Retry in {:?}", e, duration);
        },
        &mut shutdown_rx,
    )
    .await
    .with_context(|| "Failed to listen for the service")?;

    info!("Listening at {}", &bind_addr);

    let cmd = bincode::serialize(&DataChannelCmd::StartForwardUdp).unwrap();

    // Receive one data channel
    let mut conn = data_ch_rx
        .recv()
        .await
        .ok_or_else(|| anyhow!("No available data channels"))?;
    conn.write_all(&cmd).await?;

    let mut buf = [0u8; UDP_BUFFER_SIZE];
    loop {
        tokio::select! {
            // Forward inbound traffic to the client
            val = l.recv_from(&mut buf) => {
                let (n, from) = val?;
                UdpTraffic::write_slice(&mut conn, from, &buf[..n]).await?;
            },

            // Forward outbound traffic from the client to the visitor
            hdr_len = conn.read_u8() => {
                let t = UdpTraffic::read(&mut conn, hdr_len?).await?;
                l.send_to(&t.data, t.from).await?;
            }

            _ = shutdown_rx.recv() => {
                break;
            }
        }
    }

    debug!("UDP pool dropped");

    Ok(())
}

#[cfg(test)]
mod server_tests {
    use super::*;
    use std::time::Duration;
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn test_service_supervisor_lifecycle_and_grace_period() {
        // Find an open port
        let test_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = test_listener.local_addr().unwrap().port();
        drop(test_listener);

        let service = ServerServiceConfig {
            service_type: ServiceType::Tcp,
            name: "test_svc".into(),
            bind_addr: format!("127.0.0.1:{}", port),
            token: Some("secret".into()),
            ..Default::default()
        };

        let supervisor = ServiceSupervisor::<TcpTransport>::start(service);

        // Give listener brief moment to bind
        tokio::time::sleep(Duration::from_millis(50)).await;

        let (visitor_tx, mut visitor_rx) = mpsc::channel(10);
        let session = Arc::new(TunnelSession {
            session_key: [1u8; 32],
            visitor_tx,
            _phantom: std::marker::PhantomData,
        });

        // Spawn client connect before session is linked (grace period test)
        let connect_addr = format!("127.0.0.1:{}", port);
        let client_task = tokio::spawn(async move {
            TcpStream::connect(&connect_addr).await
        });

        // Link session after a small delay within the 3s grace period
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = supervisor.tunnel_tx.send(Some(session));

        // Connect must succeed
        let client_conn = client_task.await.unwrap();
        assert!(client_conn.is_ok());

        // Visitor must arrive at session's visitor_tx
        let received = tokio::time::timeout(Duration::from_secs(2), visitor_rx.recv()).await;
        assert!(received.is_ok());
        assert!(received.unwrap().is_some());

        // Cleanup
        let _ = supervisor.shutdown_tx.send(true);
    }

    #[tokio::test]
    async fn test_dispatcher_actor_backpressure_and_pairing() {
        let (visitor_tx, visitor_rx) = mpsc::channel(10);
        let (_data_ch_tx, data_ch_rx) = mpsc::channel(10);
        let (data_ch_req_tx, mut data_ch_req_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        let config = ChannelDispatcherConfig {
            max_idle_ttl: Duration::from_secs(30),
            visitor_timeout: Duration::from_secs(2),
            target_pool_size: 2,
            max_pending_visitors: 2,
        };

        let dispatcher_handle = tokio::spawn(run_tcp_channel_dispatcher::<TcpTransport>(
            visitor_rx,
            data_ch_rx,
            data_ch_req_tx,
            shutdown_rx,
            config,
        ));

        // Should receive 2 pre-warm requests initially
        let req1 = data_ch_req_rx.recv().await;
        let req2 = data_ch_req_rx.recv().await;
        assert_eq!(req1, Some(true));
        assert_eq!(req2, Some(true));

        // Create dummy socket pairs for testing visitor queueing
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let v1_connect = TcpStream::connect(addr).await.unwrap();
        let (v1_server, _) = listener.accept().await.unwrap();
        let _ = visitor_tx.send(v1_server).await;

        // Visitor 1 should trigger a replenishment request since pool is empty
        let req3 = tokio::time::timeout(Duration::from_millis(500), data_ch_req_rx.recv()).await;
        assert_eq!(req3.unwrap(), Some(true));

        // Stop dispatcher
        let _ = shutdown_tx.send(true);
        let _ = dispatcher_handle.await;
        drop(v1_connect);
    }

    #[tokio::test]
    async fn test_control_channel_bidirectional_run_and_heartbeat_ack() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let client_task = tokio::spawn(async move {
            TcpStream::connect(addr).await.unwrap()
        });
        let (server_sock, _) = listener.accept().await.unwrap();
        let mut client_sock = client_task.await.unwrap();

        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
        let (data_ch_req_tx, data_ch_req_rx) = mpsc::unbounded_channel();
        let control_channels = Arc::new(RwLock::new(ControlChannelMap::new()));
        let supervisors = Arc::new(RwLock::new(HashMap::new()));
        let session_key = [7u8; 32];
        let service_digest = [9u8; 32];

        // Insert dummy handle in control_channels to verify clean teardown removal
        {
            let (handle_shutdown_tx, _) = broadcast::channel(1);
            let (data_ch_tx, _) = mpsc::channel(1);
            let handle = ControlChannelHandle {
                _shutdown_tx: handle_shutdown_tx,
                data_ch_tx,
                service: ServerServiceConfig {
                    name: "test_svc".into(),
                    ..Default::default()
                },
                session_key,
            };
            let mut guard = control_channels.write().await;
            let _ = guard.insert(service_digest, session_key, handle);
        }

        let control_channel = ControlChannel::<TcpTransport> {
            conn: server_sock,
            shutdown_rx,
            shutdown_tx: shutdown_tx.clone(),
            data_ch_req_rx,
            heartbeat_interval: 1, // 1 second interval for fast test
            session_key,
            service_name: "test_svc".into(),
            service_digest,
            control_channels: control_channels.clone(),
            supervisors: supervisors.clone(),
        };

        let server_task = tokio::spawn(async move {
            control_channel.run().await
        });

        // 1. Client reads HeartBeat from server
        let cmd = protocol::read_control_cmd(&mut client_sock).await.unwrap();
        assert_eq!(cmd, ControlChannelCmd::HeartBeat);

        // 2. Client responds with HeartBeatAck
        let ack = bincode::serialize(&ControlChannelCmd::HeartBeatAck).unwrap();
        client_sock.write_all(&ack).await.unwrap();
        client_sock.flush().await.unwrap();

        // 3. Request data channel from server side
        let _ = data_ch_req_tx.send(true);
        let cmd2 = protocol::read_control_cmd(&mut client_sock).await.unwrap();
        assert_eq!(cmd2, ControlChannelCmd::CreateDataChannel);

        // 4. Send shutdown signal
        let _ = shutdown_tx.send(true);
        let run_res = server_task.await.unwrap();
        assert!(run_res.is_ok());

        // 5. Verify TOCTOU teardown removed the handle from control_channels
        {
            let guard = control_channels.read().await;
            assert!(guard.get2(&session_key).is_none());
        }
    }

    #[tokio::test]
    async fn test_service_supervisor_persists_without_external_shutdown_ref() {
        // Find an open port
        let test_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = test_listener.local_addr().unwrap().port();
        drop(test_listener);

        let service = ServerServiceConfig {
            service_type: ServiceType::Tcp,
            name: "test_svc_persist".into(),
            bind_addr: format!("127.0.0.1:{}", port),
            token: Some("secret".into()),
            ..Default::default()
        };

        // Initialize supervisor directly without holding any external broadcast sender,
        // exactly simulating Server::from and handle_hot_reload.
        let supervisor = ServiceSupervisor::<TcpTransport>::start(service);

        // Give listener brief moment to bind
        tokio::time::sleep(Duration::from_millis(50)).await;

        let connect_addr = format!("127.0.0.1:{}", port);

        // Verify that the listener is alive and accepting connections
        let client_conn = TcpStream::connect(&connect_addr).await;
        assert!(
            client_conn.is_ok(),
            "ServiceSupervisor ingress listener should be active and accepting connections"
        );

        // Wait an additional interval to ensure no premature teardown occurs
        tokio::time::sleep(Duration::from_millis(100)).await;

        let client_conn2 = TcpStream::connect(&connect_addr).await;
        assert!(
            client_conn2.is_ok(),
            "ServiceSupervisor ingress listener must remain alive across idle intervals"
        );

        // Explicitly trigger shutdown via supervisor.shutdown_tx
        let send_res = supervisor.shutdown_tx.send(true);
        assert!(
            send_res.is_ok(),
            "Shutdown signal must be sent successfully"
        );

        // Allow task to process shutdown signal and drop listener
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify listener has closed port and refuses new connections
        let post_shutdown_conn = TcpStream::connect(&connect_addr).await;
        assert!(
            post_shutdown_conn.is_err(),
            "Post-shutdown connection must be refused"
        );
    }
}
