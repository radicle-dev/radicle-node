use std::sync::Arc;

use gix_features::progress;
use gix_protocol::{handshake::Ref, ls_refs};
use gix_transport::{
    bstr::{BString, ByteVec},
    client::{
        self,
        capabilities::{self, Capability},
        Capabilities, Connection,
    },
};
use radicle_git_ext::transport;

pub type Error = gix_protocol::ls_refs::Error;

pub struct Config {
    pub prefixes: Vec<BString>,
}

pub fn ls_refs(config: Config, transport: Arc<Connection>) -> Result<Vec<Ref>, Error> {
    gix_protocol::ls_refs(
        transport.clone(),
        &Capabilities::default(),
        |_caps, &mut args, _feats| {
            for prefix in config.prefixes {
                // TODO: need to figure out if we have to namespace, from link-git:
                // Work around `git-upload-pack` not handling namespaces properly
                //
                // cf. https://lore.kernel.org/git/pMV5dJabxOBTD8kJBaPuWK0aS6OJhRQ7YFGwfhPCeSJEbPDrIFBza36nXBCgUCeUJWGmpjPI1rlOGvZJEh71Ruz4SqljndUwOCoBUDRHRDU=@eagain.st/
                //
                // Based on testing with git 2.25.1 in Ubuntu 20.04, this workaround is
                // not needed. Hence the checked version is lowered to 2.25.0.
                let mut arg = BString::from("ref-prefix ");
                arg.push_str(prefix);
                args.push(arg);
            }
            Ok(ls_refs::Action::Continue)
        },
        &mut progress::Discard,
    )
}
