use std::borrow::Cow;
use std::io::{self, BufRead};

use bstr::ByteSlice;
use gix_features::progress::Progress;
use gix_protocol::fetch::{self, Delegate, DelegateBlocking};
use gix_protocol::handshake::{self, Ref};
use gix_protocol::transport::Protocol;
use gix_protocol::{ls_refs, Command};
use gix_transport::bstr::{BString, ByteVec};
use gix_transport::client::{self, TransportV2Ext};

use super::{agent_name, indicate_end_of_interaction, Connection};

/// Configuration for running an ls-refs process.
///
/// See [`run`].
pub struct Config {
    /// The repository name, i.e. `/<rid>`.
    pub repo: BString,
    /// Extra parameters to pass to the ls-refs process.
    pub extra_params: Vec<(String, Option<String>)>,
    /// Ref prefixes for filtering the output of the ls-refs process.
    pub prefixes: Vec<BString>,
}

/// The Gitoxide delegate for running the ls-refs process.
struct LsRefs {
    /// Configuration for the ls-refs process.
    config: Config,
    /// The resulting references returned by the ls-refs process.
    refs: Vec<Ref>,
}

impl LsRefs {
    fn new(config: Config) -> Self {
        Self {
            config,
            refs: Vec::new(),
        }
    }
}

// FIXME: the delegate pattern will be removed in the near future and
// we should look at the fetch code being used in gix to see how we
// can migrate to the proper form of fetching.
impl DelegateBlocking for LsRefs {
    fn handshake_extra_parameters(&self) -> Vec<(String, Option<String>)> {
        self.config.extra_params.clone()
    }

    fn prepare_ls_refs(
        &mut self,
        _caps: &client::Capabilities,
        args: &mut Vec<BString>,
        _: &mut Vec<(&str, Option<Cow<'_, str>>)>,
    ) -> io::Result<ls_refs::Action> {
        for prefix in &self.config.prefixes {
            let mut arg = BString::from("ref-prefix ");
            arg.push_str(prefix);
            args.push(arg)
        }
        Ok(ls_refs::Action::Continue)
    }

    fn prepare_fetch(
        &mut self,
        _: Protocol,
        _: &client::Capabilities,
        _: &mut Vec<(&str, Option<Cow<'_, str>>)>,
        refs: &[Ref],
    ) -> io::Result<fetch::Action> {
        self.refs.extend_from_slice(refs);
        Ok(fetch::Action::Cancel)
    }

    fn negotiate(
        &mut self,
        _: &[Ref],
        _: &mut fetch::Arguments,
        _: Option<&fetch::Response>,
    ) -> io::Result<fetch::Action> {
        unreachable!("`negotiate` called even though no `fetch` command was sent")
    }
}

impl Delegate for LsRefs {
    fn receive_pack(
        &mut self,
        _: impl BufRead,
        _: impl Progress,
        _: &[Ref],
        _: &fetch::Response,
    ) -> io::Result<()> {
        unreachable!("`receive_pack` called even though no `fetch` command was sent")
    }
}

/// Run the ls-refs process using the provided `config`.
///
/// It is expected that the `handshake` was run outside of this
/// process, since it should be reused across fetch processes.
///
/// The resulting set of references are the ones returned by the
/// ls-refs process, filtered by any prefixes that were provided by
/// the `config`.
pub(crate) fn run<R, W>(
    config: Config,
    handshake: &handshake::Outcome,
    mut conn: Connection<R, W>,
    progress: &mut impl Progress,
) -> Result<Vec<Ref>, ls_refs::Error>
where
    R: io::Read,
    W: io::Write,
{
    log::trace!(target: "fetch", "Performing ls-refs: {:?}", config.prefixes);
    let mut delegate = LsRefs::new(config);
    let handshake::Outcome {
        server_protocol_version: protocol,
        capabilities,
        ..
    } = handshake;

    if protocol != &Protocol::V2 {
        return Err(ls_refs::Error::Io(io::Error::new(
            io::ErrorKind::Other,
            "expected protocol version 2",
        )));
    }

    let ls = Command::LsRefs;
    let mut features = ls.default_features(Protocol::V2, capabilities);
    // N.b. copied from gitoxide
    let mut args = vec![
        b"symrefs".as_bstr().to_owned(),
        b"peel".as_bstr().to_owned(),
    ];
    if capabilities
        .capability("ls-refs")
        .and_then(|cap| cap.supports("unborn"))
        .unwrap_or_default()
    {
        args.push("unborn".into());
    }
    let refs = match delegate.prepare_ls_refs(capabilities, &mut args, &mut features) {
        Ok(ls_refs::Action::Skip) => Vec::new(),
        Ok(ls_refs::Action::Continue) => {
            // FIXME: this is a private function
            // ls.validate_argument_prefixes_or_panic(Protocol::V2, capabilities, &args, &features);

            let agent = agent_name()?;
            features.push(("agent", Some(Cow::Owned(agent))));

            progress.step();
            progress.set_name("list refs".to_string());
            let mut remote_refs = conn.invoke(
                ls.as_str(),
                features.clone().into_iter(),
                if args.is_empty() {
                    None
                } else {
                    Some(args.into_iter())
                },
            )?;
            handshake::refs::from_v2_refs(&mut remote_refs)?
        }
        Err(err) => {
            indicate_end_of_interaction(&mut conn)?;
            return Err(err.into());
        }
    };

    Ok(refs)
}
