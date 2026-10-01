use super::beacon::validate_secret;
use super::*;
use crate::domain::DiscoverySettings;
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::util::time::unix_seconds;
use rand::rngs::OsRng;
use rand::RngCore;
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

pub struct DiscoveryService {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    status: DiscoveryStatusHandle,
}

impl DiscoveryService {
    pub fn start(
        settings: DiscoverySettings,
        context: NodeContext,
        direct_port: Option<u16>,
        secret: Option<String>,
    ) -> Result<Self, DiscoveryError> {
        if !settings.enabled {
            return Ok(Self::disabled(settings, false));
        }
        if !platform_supported() {
            return Err(DiscoveryError::UnsupportedPlatform);
        }
        validate_secret(secret.as_deref().map(str::as_bytes))?;
        if settings.port != DISCOVERY_PORT || settings.multicast_addr != MULTICAST_GROUP.to_string()
        {
            return Err(DiscoveryError::InvalidBeacon);
        }
        let interfaces = local_ipv4_interfaces();
        if interfaces.is_empty() {
            return Err(DiscoveryError::UnsupportedPlatform);
        }
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, settings.port))
            .map_err(|_| DiscoveryError::Internal)?;
        socket
            .set_nonblocking(true)
            .map_err(|_| DiscoveryError::Internal)?;
        let multicast_addr = settings
            .multicast_addr
            .parse::<Ipv4Addr>()
            .map_err(|_| DiscoveryError::InvalidBeacon)?;
        let mut multicast = false;
        for interface in &interfaces {
            if socket
                .join_multicast_v4(&multicast_addr, &interface.address)
                .is_ok()
            {
                multicast = true;
            }
        }
        let send_sockets = interfaces
            .iter()
            .filter_map(|interface| {
                let sender = UdpSocket::bind((interface.address, 0)).ok()?;
                sender.set_nonblocking(true).ok()?;
                let broadcast = if settings.broadcast && interface.broadcast.is_some() {
                    sender.set_broadcast(true).is_ok()
                } else {
                    false
                };
                Some(InterfaceSocket {
                    socket: sender,
                    address: interface.broadcast,
                    broadcast,
                })
            })
            .collect::<Vec<_>>();
        let broadcast = settings.broadcast && send_sockets.iter().any(|sender| sender.broadcast);
        if !multicast && !broadcast {
            return Err(DiscoveryError::UnsupportedPlatform);
        }
        let identity =
            NodeIdentity::load_existing(&context).map_err(|_| DiscoveryError::Internal)?;
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(DiscoverySnapshot::new(
            &settings,
            true,
            true,
            multicast,
            broadcast,
            secret.is_some(),
        )));
        let stop_for_thread = Arc::clone(&stop);
        let status_for_thread = Arc::clone(&status);
        let handle = thread::Builder::new()
            .name("omakure-lan-discovery".to_string())
            .spawn(move || {
                discovery_loop(
                    socket,
                    settings,
                    identity,
                    direct_port,
                    secret,
                    send_sockets,
                    multicast_addr,
                    multicast,
                    broadcast,
                    stop_for_thread,
                    status_for_thread,
                )
            })
            .map_err(|_| DiscoveryError::Internal)?;
        Ok(Self {
            stop,
            handle: Some(handle),
            status,
        })
    }

    pub fn disabled(settings: DiscoverySettings, supported: bool) -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            handle: None,
            status: Arc::new(Mutex::new(DiscoverySnapshot::new(
                &settings, supported, false, false, false, false,
            ))),
        }
    }

    pub fn status_without_service(
        settings: &DiscoverySettings,
        supported: bool,
        secret_configured: bool,
    ) -> DiscoveryStatus {
        DiscoverySnapshot::new(settings, supported, false, false, false, secret_configured).status
    }

    pub fn status(&self) -> DiscoveryStatusHandle {
        Arc::clone(&self.status)
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        if let Ok(mut status) = self.status.lock() {
            status.status.listening = false;
        }
    }
}

impl Drop for DiscoveryService {
    fn drop(&mut self) {
        self.stop();
    }
}

// These inputs are the complete owned lifecycle state; grouping them would
// obscure which values are process-only and which are protocol configuration.
#[allow(clippy::too_many_arguments)]
fn discovery_loop(
    socket: UdpSocket,
    settings: DiscoverySettings,
    identity: NodeIdentity,
    direct_port: Option<u16>,
    secret: Option<String>,
    send_sockets: Vec<InterfaceSocket>,
    multicast_addr: Ipv4Addr,
    multicast: bool,
    broadcast: bool,
    stop: Arc<AtomicBool>,
    status: DiscoveryStatusHandle,
) {
    let mut buffer = [0_u8; MAX_DATAGRAM_BYTES];
    let mut beacon_id = [0_u8; BEACON_ID_BYTES];
    OsRng.fill_bytes(&mut beacon_id);
    let mut sequence = 0_u64;
    let local_node_id = identity.public_status().node_id.clone();
    let mut next_send = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        let now_instant = Instant::now();
        if now_instant >= next_send {
            let now = unix_seconds();
            if let Some(direct_port) = direct_port {
                if let Ok(beacon) = Beacon::create(
                    &identity,
                    direct_port,
                    beacon_id,
                    sequence,
                    now,
                    secret.as_deref().map(str::as_bytes),
                ) {
                    if let Ok(bytes) = beacon.encode() {
                        for sender in &send_sockets {
                            if multicast {
                                let _ = sender.socket.send_to(
                                    &bytes,
                                    SocketAddr::new(IpAddr::V4(multicast_addr), settings.port),
                                );
                            }
                            if broadcast && sender.broadcast {
                                let _ = sender.socket.send_to(
                                    &bytes,
                                    SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), settings.port),
                                );
                                if let Some(address) = sender.address {
                                    let _ = sender.socket.send_to(
                                        &bytes,
                                        SocketAddr::new(IpAddr::V4(address), settings.port),
                                    );
                                }
                            }
                        }
                    }
                }
                sequence = sequence.saturating_add(1);
            }
            next_send = now_instant + BEACON_INTERVAL;
        }

        let mut received = 0;
        while received < RECEIVE_BATCH_LIMIT {
            match socket.recv_from(&mut buffer) {
                Ok((size, source)) => {
                    received += 1;
                    process_datagram(
                        &buffer[..size],
                        source,
                        &secret,
                        &status,
                        Some(&local_node_id),
                    );
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        if let Ok(mut snapshot) = status.lock() {
            snapshot.prune(unix_seconds(), Instant::now());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn process_datagram(
    bytes: &[u8],
    source: SocketAddr,
    secret: &Option<String>,
    status: &DiscoveryStatusHandle,
    local_node_id: Option<&str>,
) {
    let now_instant = Instant::now();
    let now = unix_seconds();
    let Ok(mut snapshot) = status.lock() else {
        return;
    };
    if !snapshot.admit_source(source.ip(), now_instant) {
        snapshot.status.dropped_datagrams = snapshot.status.dropped_datagrams.saturating_add(1);
        snapshot.status.last_error = Some(DiscoveryError::RateLimited.to_string());
        return;
    }
    let beacon = match Beacon::parse(bytes) {
        Ok(beacon) => beacon,
        Err(error) => {
            snapshot.status.dropped_datagrams = snapshot.status.dropped_datagrams.saturating_add(1);
            snapshot.status.last_error = Some(error.to_string());
            return;
        }
    };
    if let Err(error) = beacon.verify(now, secret.as_deref().map(str::as_bytes)) {
        snapshot.status.dropped_datagrams = snapshot.status.dropped_datagrams.saturating_add(1);
        snapshot.status.last_error = Some(error.to_string());
        return;
    }
    if local_node_id.is_some_and(|node_id| node_id == beacon.node_id) {
        return;
    }
    snapshot.accept(beacon, source.ip(), now, now_instant);
}

#[derive(Debug, Clone, Copy)]
struct InterfaceAddress {
    address: Ipv4Addr,
    broadcast: Option<Ipv4Addr>,
}

struct InterfaceSocket {
    socket: UdpSocket,
    address: Option<Ipv4Addr>,
    broadcast: bool,
}

#[cfg(unix)]
fn local_ipv4_interfaces() -> Vec<InterfaceAddress> {
    let mut result = Vec::new();
    let mut list = std::ptr::null_mut();
    // SAFETY: libc owns the linked list until freeifaddrs; each sockaddr is
    // checked for AF_INET before it is read as sockaddr_in.
    let returned = unsafe { libc::getifaddrs(&mut list) };
    if returned != 0 {
        return result;
    }
    let mut current = list;
    while !current.is_null() {
        // SAFETY: current is a node from the list returned by getifaddrs.
        let interface = unsafe { &*current };
        if !interface.ifa_addr.is_null()
            && (interface.ifa_flags & libc::IFF_UP as u32) != 0
            && unsafe { (*interface.ifa_addr).sa_family as i32 } == libc::AF_INET
        {
            // SAFETY: AF_INET guarantees sockaddr_in layout.
            let address = unsafe {
                Ipv4Addr::from(u32::from_be(
                    (*((interface.ifa_addr) as *const libc::sockaddr_in))
                        .sin_addr
                        .s_addr,
                ))
            };
            let broadcast = if !interface.ifa_netmask.is_null() {
                // SAFETY: netmask has the same family and layout as ifa_addr.
                let mask = unsafe {
                    u32::from_be(
                        (*((interface.ifa_netmask) as *const libc::sockaddr_in))
                            .sin_addr
                            .s_addr,
                    )
                };
                Some(Ipv4Addr::from((u32::from(address) & mask) | !mask))
            } else {
                None
            };
            if !result
                .iter()
                .any(|entry: &InterfaceAddress| entry.address == address)
            {
                result.push(InterfaceAddress { address, broadcast });
            }
        }
        // SAFETY: current remains within the list until the final free.
        current = unsafe { (*current).ifa_next };
    }
    // SAFETY: list was returned by getifaddrs and has not been freed.
    unsafe { libc::freeifaddrs(list) };
    result.into_iter().take(MAX_INTERFACES).collect()
}

#[cfg(not(unix))]
fn local_ipv4_interfaces() -> Vec<InterfaceAddress> {
    Vec::new()
}

pub fn platform_supported() -> bool {
    cfg!(any(target_os = "linux", target_os = "macos"))
}
