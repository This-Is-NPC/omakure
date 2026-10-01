use super::stream::time_until;
use super::{RESOLVER_CONCURRENCY, RESOLVER_QUEUE_CAPACITY};
use crate::direct_transport::TransportError;
use hickory_resolver::config::ResolverConfig;
use hickory_resolver::name_server::TokioConnectionProvider;
use hickory_resolver::TokioResolver;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio::task::JoinSet;

#[derive(Debug)]
struct ResolveRequest {
    host: String,
    port: u16,
    deadline: Instant,
    response: SyncSender<io::Result<Vec<SocketAddr>>>,
}

pub(super) struct Resolver {
    sender: mpsc::Sender<ResolveRequest>,
    stop: Arc<AtomicBool>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Resolver {
    pub(super) fn start() -> io::Result<Arc<Self>> {
        Self::start_with_config(None)
    }

    pub(super) fn start_with_config(config: Option<ResolverConfig>) -> io::Result<Arc<Self>> {
        let (sender, receiver) = mpsc::channel(RESOLVER_QUEUE_CAPACITY);
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        let (ready_sender, ready_receiver) = sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_worker = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("omakure-direct-dns".to_string())
            .spawn(move || {
                ACTIVE_RESOLVER_WORKERS.fetch_add(1, Ordering::SeqCst);
                let _worker_guard = ResolverWorkerGuard;
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_sender.send(Err(io::Error::other(error)));
                        return;
                    }
                };
                let resolver = match build_resolver(config) {
                    Ok(resolver) => resolver,
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                        return;
                    }
                };
                let _ = ready_sender.send(Ok(()));
                runtime.block_on(run_resolver(
                    receiver,
                    shutdown_receiver,
                    resolver,
                    stop_for_worker,
                ));
                // Hickory uses Tokio sockets and tasks only. run_resolver has
                // joined every request task before this owned runtime drops.
            })?;
        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Arc::new(Self {
                sender,
                stop,
                shutdown: Mutex::new(Some(shutdown_sender)),
                handle: Mutex::new(Some(handle)),
            })),
            Ok(Err(error)) => {
                let _ = handle.join();
                Err(error)
            }
            Err(_) => {
                let _ = handle.join();
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "resolver worker exited during startup",
                ))
            }
        }
    }

    pub(super) fn cancel(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut shutdown) = self.shutdown.lock() {
            if let Some(sender) = shutdown.take() {
                let _ = sender.send(());
            }
        }
    }

    pub(super) fn shutdown(&self) {
        self.cancel();
        if let Ok(mut handle) = self.handle.lock() {
            if let Some(handle) = handle.take() {
                let _ = handle.join();
            }
        }
    }

    pub(super) fn resolve(
        &self,
        endpoint: &str,
        deadline: Instant,
        stop: &AtomicBool,
    ) -> Result<Vec<SocketAddr>, TransportError> {
        let (host, port) = split_endpoint(endpoint)?;
        if let Ok(address) = host.parse::<IpAddr>() {
            return Ok(vec![SocketAddr::new(address, port)]);
        }
        if stop.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst) {
            return Err(TransportError::Internal);
        }
        let (response_sender, response_receiver) = sync_channel(1);
        let mut request = ResolveRequest {
            host,
            port,
            deadline,
            response: response_sender,
        };
        loop {
            if stop.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst) {
                return Err(TransportError::Internal);
            }
            let remaining = time_until(deadline)?;
            match self.sender.try_send(request) {
                Ok(()) => break,
                Err(mpsc::error::TrySendError::Full(returned_request)) => {
                    thread::sleep(Duration::from_millis(5).min(remaining));
                    request = returned_request;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return Err(TransportError::Internal),
            }
        }
        loop {
            if stop.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst) {
                return Err(TransportError::Internal);
            }
            let remaining = time_until(deadline)?;
            match response_receiver.recv_timeout(Duration::from_millis(25).min(remaining)) {
                Ok(result) => return result.map_err(|_| TransportError::Internal),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(TransportError::Internal)
                }
            }
        }
    }
}

impl Drop for Resolver {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(super) static ACTIVE_RESOLVER_WORKERS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
pub(super) static ACTIVE_RESOLVER_TASKS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

struct ResolverWorkerGuard;

impl Drop for ResolverWorkerGuard {
    fn drop(&mut self) {
        ACTIVE_RESOLVER_WORKERS.fetch_sub(1, Ordering::SeqCst);
    }
}

struct ResolverTaskGuard;

impl Drop for ResolverTaskGuard {
    fn drop(&mut self) {
        ACTIVE_RESOLVER_TASKS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn build_resolver(config: Option<ResolverConfig>) -> io::Result<TokioResolver> {
    let mut builder = match config {
        Some(config) => {
            TokioResolver::builder_with_config(config, TokioConnectionProvider::default())
        }
        None => {
            #[cfg(any(unix, target_os = "windows"))]
            {
                TokioResolver::builder_tokio()
                    .map_err(|error| io::Error::other(error.to_string()))?
            }
            #[cfg(not(any(unix, target_os = "windows")))]
            {
                TokioResolver::builder_with_config(
                    ResolverConfig::default(),
                    TokioConnectionProvider::default(),
                )
            }
        }
    };
    // Static peers can move to a new address while this service stays up.
    // Each reconnect attempt must observe DNS again, regardless of the TTL
    // returned before the peer restarted.
    builder.options_mut().cache_size = 0;
    Ok(builder.build())
}

async fn run_resolver(
    mut receiver: mpsc::Receiver<ResolveRequest>,
    mut shutdown: oneshot::Receiver<()>,
    resolver: TokioResolver,
    stop: Arc<AtomicBool>,
) {
    let permits = Arc::new(Semaphore::new(RESOLVER_CONCURRENCY));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            request = receiver.recv() => {
                let Some(request) = request else { break };
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let remaining = match request.deadline.checked_duration_since(Instant::now()) {
                    Some(remaining) if !remaining.is_zero() => remaining,
                    _ => {
                        let _ = request.response.send(Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "resolver deadline",
                        )));
                        continue;
                    }
                };
                let permit = tokio::select! {
                    _ = &mut shutdown => break,
                    permit = tokio::time::timeout(remaining, Arc::clone(&permits).acquire_owned()) => {
                        match permit {
                            Ok(Ok(permit)) => permit,
                            _ => {
                                let _ = request.response.send(Err(io::Error::new(
                                    io::ErrorKind::TimedOut,
                                    "resolver queue deadline",
                                )));
                                continue;
                            }
                        }
                    }
                };
                let resolver = resolver.clone();
                ACTIVE_RESOLVER_TASKS.fetch_add(1, Ordering::SeqCst);
                tasks.spawn(async move {
                    let _task_guard = ResolverTaskGuard;
                    let host = request.host;
                    let port = request.port;
                    let result = tokio::time::timeout(remaining, async move {
                        let lookup = resolver
                            .lookup_ip(host)
                            .await
                            .map_err(|error| io::Error::other(error.to_string()))?;
                        let mut addresses = Vec::new();
                        for ip in lookup.iter() {
                            let address = SocketAddr::new(ip, port);
                            if !addresses.contains(&address) {
                                addresses.push(address);
                            }
                        }
                        if addresses.is_empty() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                "resolver returned no addresses",
                            ));
                        }
                        Ok(addresses)
                    })
                    .await
                    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "resolver timeout"))
                    .and_then(|result| result);
                    let _ = request.response.send(result);
                    drop(permit);
                });
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}

fn split_endpoint(endpoint: &str) -> Result<(String, u16), TransportError> {
    if let Ok(address) = endpoint.parse::<SocketAddr>() {
        return Ok((address.ip().to_string(), address.port()));
    }
    if let Some(host_and_port) = endpoint.strip_prefix('[') {
        let Some((host, port)) = host_and_port.split_once("]:") else {
            return Err(TransportError::Internal);
        };
        return port
            .parse::<u16>()
            .map(|port| (host.to_string(), port))
            .map_err(|_| TransportError::Internal);
    }
    let Some((host, port)) = endpoint.rsplit_once(':') else {
        return Err(TransportError::Internal);
    };
    if host.is_empty() {
        return Err(TransportError::Internal);
    }
    port.parse::<u16>()
        .map(|port| (host.to_string(), port))
        .map_err(|_| TransportError::Internal)
}
