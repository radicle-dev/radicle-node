use std::{
    borrow::Cow,
    io::{self, BufRead},
    path::PathBuf,
    sync::{self, atomic::AtomicBool, Arc, Mutex, RwLock},
};

use gix_features::{
    interrupt,
    progress::{prodash::progress, Progress},
};
use gix_hash::ObjectId;
use gix_pack as pack;
use gix_protocol::{
    fetch::{self, Delegate, DelegateBlocking},
    handshake::{self, Ref},
    ls_refs, FetchConnection,
};
use gix_transport::{
    bstr::BString,
    client::{self, Connection},
    Protocol,
};

pub type Error = gix_protocol::fetch::Error;

pub struct PackWriter {
    git_dir: PathBuf,
    interrupt: Arc<AtomicBool>,
    max_threads: Option<usize>,
}

impl PackWriter {
    pub fn write_pack(
        &self,
        pack: impl BufRead,
        progress: impl Progress,
    ) -> io::Result<pack::bundle::write::Outcome> {
        let options = pack::bundle::write::Options {
            thread_limit: self.max_threads,
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
        let thinkener = gix_odb::Store::at_opts(self.git_dir.join("objects"), [], options)?;
        pack::Bundle::write_to_directory(
            pack,
            Some(self.git_dir.join("objects").join("pack")),
            progress,
            &self.interrupt,
            Some(Box::new(move |oid, buf| thickener.find_object(oid, buf))),
            options,
        )
    }
}

pub struct Config {
    pub wants: Vec<ObjectId>,
    pub haves: Vec<ObjectId>,
}

pub struct Fetch {
    config: Config,
    pack_writer: PackWriter,
    out: Arc<Mutex<FetchOut>>,
    set: Arc<AtomicBool>,
}

pub struct FetchOut {
    refs: Vec<Ref>,
    pack: Option<pack::bundle::write::Outcome>,
}

impl Delegate for Fetch {
    fn receive_pack(
        &mut self,
        input: impl io::BufRead,
        progress: impl Progress,
        refs: &[handshake::Ref],
        previous_response: &fetch::Response,
    ) -> io::Result<()> {
        let mut out = self
            .out
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "fetch delegate locked"))?;
        out.refs.extend(previous_response.wanted_refs().iter().map(
            |fetch::response::WantedRef { id, path }| Ref::Direct {
                full_ref_name: path.clone(),
                object: *id,
            },
        ));
        let out = self.pack_writer.write_pack(input, progress)?;
        out.pack = Some(out);
        Ok(())
    }
}

impl DelegateBlocking for Fetch {
    fn negotiate(
        &mut self,
        _refs: &[handshake::Ref],
        arguments: &mut fetch::Arguments,
        _previous_response: Option<&fetch::Response>,
    ) -> io::Result<fetch::Action> {
        for oid in self.config.wants {
            arguments.want(oid);
        }

        for oid in self.config.haves {
            arguments.have(oid);
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
        // We perform ls-refs elsewhere
        Ok(ls_refs::Action::Skip)
    }

    fn prepare_fetch(
        &mut self,
        _version: Protocol,
        server: &client::Capabilities,
        _features: &mut Vec<(&str, Option<Cow<'_, str>>)>,
        _refs: &[handshake::Ref],
    ) -> io::Result<fetch::Action> {
        if self.wants.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "empty fetch"));
        }
        Ok(fetch::Action::Continue)
    }
}

pub fn fetch(config: Config, transport: Arc<Connection>) -> Result<Vec<Ref>, Error> {
    // TODO(finto): not sure what the agent should be, possibly radicle-node + version
    let agent = "radicle-node";

    // TODO: I think this is supposed to be used in a threaded
    // environment so it might need to be passed in via the caller.
    let interrupt = Arc::new(AtomicBool::new(false));

    let set = Arc::new(AtomicBool::new(false));
    let delegate = Fetch {
        config,
        pack_writer: PackWriter {
            git_dir: todo!(),
            interrupt,
            max_threads: todo!(),
        },
        out: FetchOut {
            refs: Vec::new(),
            pack: None,
        },
        set: set.clone(),
    };

    // N.b. delegate gets consumed so we attempt to read the output.
    let out = delegate.out.clone();
    gix_protocol::fetch(
        transport.clone(),
        delegate,
        |_action| Ok(None),
        progress::Discard,
        FetchConnection::AllowReuse,
        agent,
    )?;

    // N.b wait for the delegate's outputs to be set
    while !set.load(sync::atomic::Ordering::Acquire) {
        std::thread::sleep(std::time::Duration::from_millis(500))
    }

    Ok(out.into().refs)
}
