use super::error::DirectServiceError;
use super::{CONNECT_TIMEOUT, HANDSHAKE_TIMEOUT, HEADER_TIMEOUT};
use crate::direct_transport::{Frame, TransportError};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Time left before `deadline`; zero once it has just been reached.
pub(super) fn time_until(deadline: Instant) -> Result<Duration, TransportError> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or(TransportError::Internal)
}

/// Time left before `deadline`, refusing a deadline already reached.
pub(super) fn deadline_timeout(deadline: Instant) -> Result<Duration, TransportError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(TransportError::Internal)
}

pub(super) fn initiator_deadline(started: Instant) -> Instant {
    started + CONNECT_TIMEOUT
}

pub(super) fn set_stream_timeouts(
    stream: &TcpStream,
    deadline: Instant,
) -> Result<(), TransportError> {
    let timeout = deadline_timeout(deadline)?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|_| TransportError::Internal)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|_| TransportError::Internal)
}

pub(super) fn read_frame(
    stream: &mut TcpStream,
    deadline: Instant,
) -> Result<Vec<u8>, DirectServiceError> {
    let mut prefix = [0u8; 4];
    read_bytes_until(
        stream,
        &mut prefix,
        deadline.min(Instant::now() + HEADER_TIMEOUT),
    )?;
    let length = u32::from_be_bytes(prefix) as usize;
    if !(4..=crate::direct_transport::MAX_FRAME_LENGTH).contains(&length) {
        return Err(TransportError::MessageTooLarge.into());
    }
    let mut frame = Vec::with_capacity(length + 4);
    frame.extend_from_slice(&prefix);
    frame.resize(length + 4, 0);
    let started_kib = length.saturating_add(65_535) / 65_536;
    let body_timeout = Duration::from_secs(1)
        .saturating_add(Duration::from_secs(started_kib as u64))
        .min(HANDSHAKE_TIMEOUT);
    read_bytes_until(
        stream,
        &mut frame[4..],
        deadline.min(Instant::now() + body_timeout),
    )?;
    Frame::parse(&frame)?;
    Ok(frame)
}

pub(super) fn write_bytes(
    stream: &mut TcpStream,
    bytes: &[u8],
    deadline: Instant,
) -> Result<(), DirectServiceError> {
    transfer_until(bytes.len(), deadline, |offset, timeout| {
        stream.set_write_timeout(Some(timeout))?;
        match stream.write(&bytes[offset..])? {
            0 => Err(io::ErrorKind::WriteZero.into()),
            written => Ok(written),
        }
    })?;
    Ok(())
}

fn read_bytes_until(stream: &mut TcpStream, bytes: &mut [u8], deadline: Instant) -> io::Result<()> {
    transfer_until(bytes.len(), deadline, |offset, timeout| {
        stream.set_read_timeout(Some(timeout))?;
        match stream.read(&mut bytes[offset..])? {
            0 => Err(io::ErrorKind::UnexpectedEof.into()),
            read => Ok(read),
        }
    })
}

/// Socket timeouts bound one syscall, not a sequence of partial reads/writes.
/// Keep the same absolute budget across progress and interrupted syscalls.
pub(super) fn transfer_until(
    length: usize,
    deadline: Instant,
    mut transfer: impl FnMut(usize, Duration) -> io::Result<usize>,
) -> io::Result<()> {
    let mut offset = 0;
    while offset < length {
        let timeout = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(io::ErrorKind::TimedOut)?;
        match transfer(offset, timeout) {
            Ok(count) => offset += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    Ok(())
}
