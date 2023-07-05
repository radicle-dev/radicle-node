use std::convert::Infallible;
use std::io::{Read, Write};
use std::ops::Deref;
use std::{fmt, io, time};

use crossbeam_channel as chan;
use radicle::node::NodeId;

use crate::runtime::Handle;
use crate::wire::StreamId;

/// A reader and writer pair that can be used in the fetch protocol.
///
/// It implements [`radicle::fetch::transport::ConnectionStream`] to
/// provide its underlying channels for reading and writing.
pub struct Tunnel {
    receiver: ChannelReader,
    sender: ChannelFlushWriter,
}

impl Tunnel {
    pub fn new(handle: Handle, channels: Channels, remote: NodeId) -> Self {
        Self {
            receiver: channels.receiver,
            sender: ChannelFlushWriter {
                writer: channels.sender,
                handle,
                remote,
            },
        }
    }

    pub fn split(&mut self) -> (&mut ChannelReader, &mut ChannelFlushWriter) {
        (&mut self.receiver, &mut self.sender)
    }
}

impl radicle::fetch::transport::ConnectionStream for Tunnel {
    type Read = ChannelReader;
    type Write = ChannelFlushWriter;
    type Error = Infallible;

    fn open(&mut self) -> Result<(&mut Self::Read, &mut Self::Write), Self::Error> {
        Ok((&mut self.receiver, &mut self.sender))
    }
}

/// Data that can be sent and received on worker channels.
pub enum ChannelEvent<T = Vec<u8>> {
    /// Git protocol data.
    Data(T),
    /// A request to close the channel.
    Close,
    /// A signal that the git protocol has ended, eg. when the remote fetch closes the
    /// connection.
    Eof,
}

impl<T> From<T> for ChannelEvent<T> {
    fn from(value: T) -> Self {
        Self::Data(value)
    }
}

impl<T> fmt::Debug for ChannelEvent<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data(_) => write!(f, "ChannelEvent::Data(..)"),
            Self::Close => write!(f, "ChannelEvent::Close"),
            Self::Eof => write!(f, "ChannelEvent::Eof"),
        }
    }
}

/// Worker channels for communicating through the git stream with the remote.
pub struct Channels<T = Vec<u8>> {
    sender: ChannelWriter<T>,
    receiver: ChannelReader<T>,
}

impl<T: AsRef<[u8]>> Channels<T> {
    pub fn new(
        sender: chan::Sender<ChannelEvent<T>>,
        receiver: chan::Receiver<ChannelEvent<T>>,
        stream: StreamId,
        timeout: time::Duration,
    ) -> Self {
        let sender = ChannelWriter {
            sender,
            stream,
            timeout,
        };
        let receiver = ChannelReader::new(receiver, timeout);

        Self { sender, receiver }
    }

    pub fn pair(
        stream: StreamId,
        timeout: time::Duration,
    ) -> io::Result<(Channels<T>, Channels<T>)> {
        let (l_send, r_recv) = chan::unbounded::<ChannelEvent<T>>();
        let (r_send, l_recv) = chan::unbounded::<ChannelEvent<T>>();

        let l = Channels::new(l_send, l_recv, stream, timeout);
        let r = Channels::new(r_send, r_recv, stream, timeout);

        Ok((l, r))
    }

    pub fn try_iter(&self) -> impl Iterator<Item = ChannelEvent<T>> + '_ {
        self.receiver.try_iter()
    }

    pub fn send(&self, event: ChannelEvent<T>) -> io::Result<()> {
        self.sender.send(event)
    }

    pub fn close(self) -> Result<(), chan::SendError<ChannelEvent<T>>> {
        self.sender.close()
    }
}

/// Wraps a [`chan::Receiver`] and provides it with [`io::Read`].
#[derive(Clone)]
pub struct ChannelReader<T = Vec<u8>> {
    buffer: io::Cursor<Vec<u8>>,
    receiver: chan::Receiver<ChannelEvent<T>>,
    timeout: time::Duration,
}

impl<T> Deref for ChannelReader<T> {
    type Target = chan::Receiver<ChannelEvent<T>>;

    fn deref(&self) -> &Self::Target {
        &self.receiver
    }
}

impl<T: AsRef<[u8]>> ChannelReader<T> {
    pub fn new(receiver: chan::Receiver<ChannelEvent<T>>, timeout: time::Duration) -> Self {
        Self {
            buffer: io::Cursor::new(Vec::new()),
            receiver,
            timeout,
        }
    }
}

impl Read for ChannelReader<Vec<u8>> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.buffer.read(buf)?;
        if read > 0 {
            return Ok(read);
        }

        match self.receiver.recv_timeout(self.timeout) {
            Ok(ChannelEvent::Data(data)) => {
                self.buffer = io::Cursor::new(data);
                self.buffer.read(buf)
            }
            Ok(ChannelEvent::Eof) => Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(ChannelEvent::Close) => Err(io::ErrorKind::ConnectionReset.into()),

            Err(chan::RecvTimeoutError::Timeout) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "error reading from stream: channel timed out",
            )),
            Err(chan::RecvTimeoutError::Disconnected) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "error reading from stream: channel is disconnected",
            )),
        }
    }
}

/// Wraps a [`chan::Sender`] and provides it with [`io::Write`].
#[derive(Clone)]
struct ChannelWriter<T = Vec<u8>> {
    sender: chan::Sender<ChannelEvent<T>>,
    stream: StreamId,
    timeout: time::Duration,
}

/// Wraps a [`ChannelWriter`] alongside the associated [`Handle`] and [`NodeId`].
///
/// This allows the channel to [`Write::flush`] when calling
/// [`Write::write_all`], which is necessary to signal to the
/// controller to send the wire data.
pub struct ChannelFlushWriter<T = Vec<u8>> {
    writer: ChannelWriter<T>,
    handle: Handle,
    remote: NodeId,
}

impl Write for ChannelFlushWriter<Vec<u8>> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = buf.len();
        self.writer.send(buf.to_vec())?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.handle.flush(self.remote, self.writer.stream)
    }

    fn write_all(&mut self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            match self.write(buf) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to write whole buffer",
                    ));
                }
                Ok(n) => buf = &buf[n..],
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        self.flush()
    }
}

impl<T: AsRef<[u8]>> ChannelWriter<T> {
    pub fn send(&self, event: impl Into<ChannelEvent<T>>) -> io::Result<()> {
        match self.sender.send_timeout(event.into(), self.timeout) {
            Ok(()) => Ok(()),
            Err(chan::SendTimeoutError::Timeout(_)) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "error writing to stream: channel timed out",
            )),
            Err(chan::SendTimeoutError::Disconnected(_)) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "error writing to stream: channel is disconnected",
            )),
        }
    }

    /// Permanently close this stream.
    pub fn close(self) -> Result<(), chan::SendError<ChannelEvent<T>>> {
        self.sender.send(ChannelEvent::Close)
    }
}
