use std::io::{self, Read};
use std::net::TcpStream;
use std::ops::RangeInclusive;

#[derive(Clone, Copy)]
pub enum ReadStage {
    Prefix,
    Body,
}

pub fn read_frame(
    stream: &mut TcpStream,
    allowed_length: Option<RangeInclusive<usize>>,
    mut read_stage: Option<&mut ReadStage>,
) -> io::Result<Vec<u8>> {
    let mut prefix = [0_u8; 4];
    if let Some(stage) = &mut read_stage {
        **stage = ReadStage::Prefix;
    }
    stream.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if allowed_length.is_some_and(|allowed| !allowed.contains(&length)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {length} is outside the shipped bound"),
        ));
    }
    let mut encoded = vec![0_u8; length + 4];
    encoded[..4].copy_from_slice(&prefix);
    if let Some(stage) = &mut read_stage {
        **stage = ReadStage::Body;
    }
    stream.read_exact(&mut encoded[4..])?;
    Ok(encoded)
}
