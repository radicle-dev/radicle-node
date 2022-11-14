use std::convert::Infallible;

/// Data processed by the transcoder, which may be either a decoded data block
/// or an output which must be sent to the remote peer (used during handshake).
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum DecodedData {
    /// Data represent a decoded fragment which must be processed by a local peer.
    Local(Vec<u8>),
    /// Data represent a fragment which must be sent to the remote peer.
    Remote(Vec<u8>),
}

/// Trait allowing transcoding the stream using some form of stream encryption
/// and/or encoding.
pub trait Transcode {
    /// Errors generated by the transcoder.
    type Error: std::error::Error;

    /// Decodes data received from the remote peer and update the internal state
    /// of the transcoder, either returning response which must be sent to the
    /// remote (see [`DecodedData::Remote`]) or data which should be processed
    /// by the local peer.
    fn input(&mut self, data: &[u8]) -> Result<DecodedData, Self::Error>;

    /// Encodes data before sending them to the remote peer.
    fn output(&mut self, data: Vec<u8>) -> Result<Vec<u8>, Self::Error>;
}

/// Transcoder which does nothing.
#[derive(Debug, Default)]
pub struct PlainTranscoder;

impl Transcode for PlainTranscoder {
    type Error = Infallible;

    fn input(&mut self, data: &[u8]) -> Result<DecodedData, Self::Error> {
        Ok(DecodedData::Local(data.to_vec()))
    }

    fn output(&mut self, data: Vec<u8>) -> Result<Vec<u8>, Self::Error> {
        Ok(data)
    }
}
