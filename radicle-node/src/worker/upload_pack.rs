use std::io;
use std::io::Write;
use std::process::{Command, ExitStatus, Stdio};
use std::str::FromStr;

use gix_packetline as packetline;
use gix_packetline::PacketLineRef;
use radicle::node::NodeId;
use radicle::{storage::ReadStorage, Storage};

use crate::runtime::thread;

#[derive(Debug, PartialEq, Eq)]
pub struct Header {
    pub path: String,
    pub host: Option<(String, Option<u16>)>,
    pub extra: Vec<(String, Option<String>)>,
}

impl FromStr for Header {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s
            .strip_prefix("git-upload-pack ")
            .ok_or("unsupported service")?
            .split_terminator('\0');

        let path = parts.next().ok_or("missing path").and_then(|path| {
            if path.is_empty() {
                Err("empty path")
            } else {
                Ok(path.to_owned())
            }
        })?;
        let host = match parts.next() {
            None | Some("") => None,
            Some(host) => match host.strip_prefix("host=") {
                None => return Err("invalid host"),
                Some(host) => match host.split_once(':') {
                    None => Some((host.to_owned(), None)),
                    Some((host, port)) => {
                        let port = port.parse::<u16>().or(Err("invalid port"))?;
                        Some((host.to_owned(), Some(port)))
                    }
                },
            },
        };
        let extra = parts
            .skip_while(|part| part.is_empty())
            .map(|part| match part.split_once('=') {
                None => (part.to_owned(), None),
                Some((k, v)) => (k.to_owned(), Some(v.to_owned())),
            })
            .collect();

        Ok(Self { path, host, extra })
    }
}

pub fn header<R>(mut recv: R) -> io::Result<(Header, R)>
where
    R: io::Read + Send,
{
    log::debug!(target: "worker", "upload-pack waiting for header");
    let mut pktline = packetline::StreamingPeekableIter::new(recv, &[]);
    let pkt = pktline
        .read_line()
        .ok_or_else(|| invalid_data("missing header"))?
        .map_err(invalid_data)?
        .map_err(invalid_data)?;
    let header: Header = match pkt {
        PacketLineRef::Data(data) => std::str::from_utf8(data)
            .map_err(invalid_data)?
            .parse()
            .map_err(invalid_data),
        _ => Err(invalid_data("not a header packet")),
    }?;
    recv = pktline.into_inner();

    log::debug!(
        target: "worker",
        "upload-pack received header path={:?}, host={:?}",
        header.path,
        header.host
    );

    Ok((header, recv))
}

pub fn upload_pack<R, W>(
    nid: &NodeId,
    storage: &Storage,
    header: &Header,
    mut recv: R,
    mut send: W,
) -> io::Result<ExitStatus>
where
    R: io::Read + Send,
    W: io::Write + Send,
{
    let namespace = header
        .path
        .strip_prefix("rad:")
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| header.path.clone());
    let protocol_version = header
        .extra
        .iter()
        .find_map(|kv| match kv {
            (ref k, Some(v)) if k == "version" => {
                let version = match v.as_str() {
                    "2" => 2,
                    "1" => 1,
                    _ => 0,
                };
                Some(version)
            }
            _ => None,
        })
        .unwrap_or(0);

    let git_dir = {
        let rid = namespace
            .strip_prefix('/')
            .unwrap_or(&namespace)
            .parse()
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        let repo = storage
            .repository(rid)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        repo.backend.path().to_path_buf()
    };

    let mut child = {
        let mut cmd = Command::new("git");
        cmd.current_dir(git_dir)
            .env_clear()
            .envs(std::env::vars().filter(|(key, _)| key == "PATH" || key.starts_with("GIT_TRACE")))
            .env("GIT_PROTOCOL", format!("version={}", protocol_version))
            .args([
                "-c",
                "uploadpack.allowanysha1inwant=true",
                "-c",
                "uploadpack.allowrefinwant=true",
                "-c",
                "lsrefs.unborn=ignore",
                "upload-pack",
                "--strict",
                ".",
            ])
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .stderr(Stdio::inherit());

        cmd.spawn()?
    };

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = io::BufReader::new(child.stdout.take().unwrap());
    thread::scope(|s| {
        thread::spawn_scoped(nid, "upload-pack", s, || {
            // N.b. we indefinitely copy stdout to the sender,
            // i.e. there's no need for a loop.
            match io::copy(&mut stdout, &mut send) {
                Ok(_) => {}
                Err(e) => {
                    log::error!(target: "worker", "Worker channel disconnected; aborting: {e}");
                }
            }
        });

        let reader = thread::spawn_scoped(nid, "upload-pack", s, || {
            let mut buffer = [0; u16::MAX as usize + 1];
            loop {
                match recv.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        // N.b. signal that the fetch process has finished
                        // TODO: probably a better way to signal EOF
                        if &buffer[..n] == b"heartwood/finished" {
                            log::debug!(target: "worker", "exiting upload-pack receive thread");
                            break;
                        }

                        if let Err(e) = stdin.write_all(&buffer[..n]) {
                            log::warn!(target: "worker", "upload-pack stdin write error: {e}");
                            break;
                        }
                    }
                    Err(e) => {
                        log::error!(target: "worker", "upload-pack channel read error: {e}");
                        break;
                    }
                }
            }
        });

        // N.b. we only care if the `reader` is finished. We then kill
        // the child which will end the thread for the sender.
        loop {
            if reader.is_finished() {
                child.kill()?;
                break;
            } else {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        Ok::<_, io::Error>(())
    })?;

    let status = child.wait()?;
    Ok(status)
}

fn invalid_data<E>(inner: E) -> io::Error
where
    E: Into<Box<dyn std::error::Error + Sync + Send>>,
{
    io::Error::new(io::ErrorKind::InvalidData, inner)
}
