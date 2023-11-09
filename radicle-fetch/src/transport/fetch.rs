use std::{
    borrow::Cow,
    io::{self, BufRead},
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};

use gix_features::progress::{DynNestedProgress, NestedProgress, Progress};
use gix_pack as pack;
use gix_protocol::{
    fetch::{self, Delegate, DelegateBlocking},
    handshake::{self, Ref},
    ls_refs, FetchConnection,
};
use gix_transport::{bstr::BString, client, Protocol};

use super::{agent_name, indicate_end_of_interaction, Connection, WantsHaves};

pub type Error = gix_protocol::fetch::Error;

pub mod error {
    use std::io;

    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum PackWriter {
        #[error(transparent)]
        Io(#[from] io::Error),
        #[error(transparent)]
        Write(#[from] gix_pack::bundle::write::Error),
    }
}

/// Configuration for writing a packfile.
pub struct PackWriter {
    /// The repository path for writing the packfile to. Note this is
    /// the root of the Git repository, e.g. the `.git` folder.
    pub git_dir: PathBuf,
    /// `interrupt` is checked regularly and when true, the whole
    /// operation will stop.
    pub interrupt: Arc<AtomicBool>,
}

impl PackWriter {
    /// Write the packfile read from `pack` to the `objects/pack`
    /// directory.
    pub fn write_pack(
        &self,
        mut pack: impl BufRead,
        progress: &mut impl DynNestedProgress,
    ) -> Result<pack::bundle::write::Outcome, error::PackWriter> {
        use gix_odb::FindExt as _;

        let options = pack::bundle::write::Options {
            // N.b. use all cores. Can make configurable if needed
            // later.
            thread_limit: None,
            iteration_mode: pack::data::input::Mode::Verify,
            index_version: pack::index::Version::V2,
            object_hash: gix_hash::Kind::Sha1,
        };
        let odb_opts = gix_odb::store::init::Options {
            slots: gix_odb::store::init::Slots::default(),
            object_hash: gix_hash::Kind::Sha1,
            use_multi_pack_index: true,
            current_dir: Some(self.git_dir.clone()),
        };
        let thickener = Arc::new(gix_odb::Store::at_opts(
            self.git_dir.join("objects"),
            &mut [].into_iter(),
            odb_opts,
        )?);
        let thickener = thickener.to_handle_arc();
        Ok(pack::Bundle::write_to_directory(
            &mut pack,
            Some(&self.git_dir.join("objects").join("pack")),
            progress,
            &self.interrupt,
            Some(Box::new(move |oid, buf| thickener.find(&oid, buf).ok())),
            options,
        )?)
    }
}

/// The fetch [`Delegate`] that negotiates the fetch with the
/// server-side.
pub struct Fetch {
    wants_haves: WantsHaves,
    pack_writer: PackWriter,
    out: FetchOut,
}

/// The result of running a fetch via [`run`].
pub struct FetchOut {
    pub refs: Vec<Ref>,
    pub pack: Option<pack::bundle::write::Outcome>,
}

// FIXME: the delegate pattern will be removed in the near future and
// we should look at the fetch code being used in gix to see how we
// can migrate to the proper form of fetching.
impl<'a> Delegate for &'a mut Fetch {
    fn receive_pack(
        &mut self,
        input: impl io::BufRead,
        mut progress: impl NestedProgress + 'static,
        _refs: &[handshake::Ref],
        previous_response: &fetch::Response,
    ) -> io::Result<()> {
        self.out
            .refs
            .extend(previous_response.wanted_refs().iter().map(
                |fetch::response::WantedRef { id, path }| Ref::Direct {
                    full_ref_name: path.clone(),
                    object: *id,
                },
            ));
        let pack = self
            .pack_writer
            .write_pack(input, &mut progress)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        self.out.pack = Some(pack);
        Ok(())
    }
}

impl<'a> DelegateBlocking for &'a mut Fetch {
    fn negotiate(
        &mut self,
        _refs: &[handshake::Ref],
        arguments: &mut fetch::Arguments,
        _previous_response: Option<&fetch::Response>,
    ) -> io::Result<fetch::Action> {
        use crate::git::oid;

        for oid in &self.wants_haves.wants {
            arguments.want(oid::to_object_id(*oid));
        }

        for oid in &self.wants_haves.haves {
            arguments.have(oid::to_object_id(*oid));
        }

        // N.b. sends `done` packet
        Ok(fetch::Action::Cancel)
    }

    fn prepare_ls_refs(
        &mut self,
        _server: &client::Capabilities,
        _arguments: &mut Vec<BString>,
        _features: &mut Vec<(&str, Option<Cow<'_, str>>)>,
    ) -> io::Result<ls_refs::Action> {
        // N.b. we performed ls-refs before the fetch already.
        Ok(ls_refs::Action::Skip)
    }

    fn prepare_fetch(
        &mut self,
        _version: Protocol,
        _server: &client::Capabilities,
        _features: &mut Vec<(&str, Option<Cow<'_, str>>)>,
        _refs: &[handshake::Ref],
    ) -> io::Result<fetch::Action> {
        if self.wants_haves.wants.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "empty fetch"));
        }
        Ok(fetch::Action::Continue)
    }
}

/// Run the fetch process using the provided `config` and
/// `pack_writer` configuration.
///
/// It is expected that the `handshake` was run outside of this
/// process, since it should be reused across fetch processes.
#[allow(clippy::result_large_err)]
pub(crate) fn run<P, R, W>(
    wants_haves: WantsHaves,
    pack_writer: PackWriter,
    handshake: &handshake::Outcome,
    mut conn: Connection<R, W>,
    mut progress: P,
) -> Result<FetchOut, Error>
where
    P: Progress + NestedProgress + 'static,
    R: io::Read,
    W: io::Write,
{
    log::trace!(target: "fetch", "Performing fetch");

    let mut delegate = Fetch {
        wants_haves,
        pack_writer,
        out: FetchOut {
            refs: Vec::new(),
            pack: None,
        },
    };

    let handshake::Outcome {
        server_protocol_version: protocol,
        refs: _refs,
        capabilities,
    } = handshake;
    let agent = agent_name()?;
    let fetch = gix_protocol::Command::Fetch;

    let mut features = fetch.default_features(*protocol, capabilities);
    match (&mut delegate).prepare_fetch(*protocol, capabilities, &mut features, &[]) {
        Ok(fetch::Action::Continue) => {
            // FIXME: this is a private function in gitoxide
            // fetch.validate_argument_prefixes_or_panic(protocol, &capabilities, &[], &features)
        }
        // N.b. we always return Action::Continue
        Ok(fetch::Action::Cancel) => unreachable!(),
        Err(err) => {
            indicate_end_of_interaction(&mut conn)?;
            return Err(err.into());
        }
    }

    gix_protocol::fetch::Response::check_required_features(*protocol, &features)?;
    let sideband_all = features.iter().any(|(n, _)| *n == "sideband-all");
    features.push(("agent", Some(Cow::Owned(agent))));
    let mut args = fetch::Arguments::new(*protocol, features);

    let mut previous_response = None::<fetch::Response>;
    let mut round = 1;
    'negotiation: loop {
        progress.step();
        progress.set_name(format!("negotiate (round {round})"));
        round += 1;
        let action = (&mut delegate).negotiate(&[], &mut args, previous_response.as_ref())?;
        let mut reader = args.send(&mut conn, action == fetch::Action::Cancel)?;
        if sideband_all {
            setup_remote_progress(&mut progress, &mut reader);
        }
        let response = fetch::Response::from_line_reader(*protocol, &mut reader, true)?;
        previous_response = if response.has_pack() {
            progress.step();
            progress.set_name("receiving pack".to_string());
            if !sideband_all {
                setup_remote_progress(&mut progress, &mut reader);
            }
            (&mut delegate).receive_pack(reader, progress, &[], &response)?;
            break 'negotiation;
        } else {
            match action {
                fetch::Action::Cancel => break 'negotiation,
                fetch::Action::Continue => Some(response),
            }
        }
    }
    if matches!(protocol, Protocol::V2)
        && matches!(conn.mode, FetchConnection::TerminateOnSuccessfulCompletion)
    {
        indicate_end_of_interaction(&mut conn)?;
    }

    // N.b. the flush packet is never read in the packfile parser. To
    // remedy this, we read from our connection until we see it ensure
    // that there is no leftover data.
    let (mut r, _) = conn.inner.into_inner();
    let mut buf = [0; 4096];

    loop {
        match r.read(&mut buf)? {
            0 => break,
            n => match std::str::from_utf8(&buf[..n]) {
                Ok(progress) => {
                    let lines = progress.split('\n');
                    if lines.into_iter().any(|line| line == "0000") {
                        break;
                    }
                }
                Err(e) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("found leftover packfile bytes: {e}"),
                    )
                    .into());
                }
            },
        }
    }

    log::trace!(target: "fetch", "fetched refs: {:?}", delegate.out.refs);
    Ok(delegate.out)
}

fn setup_remote_progress<P>(
    progress: &mut P,
    reader: &mut Box<dyn gix_transport::client::ExtendedBufRead + Unpin + '_>,
) where
    P: Progress + NestedProgress + 'static,
{
    reader.set_progress_handler(Some(Box::new({
        let mut remote_progress = progress.add_child("remote");
        move |is_err: bool, data: &[u8]| {
            gix_protocol::RemoteProgress::translate_to_progress(is_err, data, &mut remote_progress);
            gix_transport::packetline::read::ProgressAction::Continue
        }
    }) as gix_transport::client::HandleProgress));
}
