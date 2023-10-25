use std::ffi::OsString;

use anyhow::anyhow;

use radicle::identity::Id;
use radicle::node;
use radicle::node::Handle as _;
use radicle::storage;
use radicle::storage::WriteStorage;
use radicle::Profile;

use crate::git;
use crate::terminal as term;
use crate::terminal::args::{Args, Error, Help};

pub const HELP: Help = Help {
    name: "rm",
    description: "Remove a project",
    version: env!("CARGO_PKG_VERSION"),
    usage: r#"
Usage

    rad rm <rid> [<option>...]

    Removes a repository from storage. The repository is also untracked, if possible.

Options

    --no-confirm        Do not ask for confirmation before removal (default: false)
    --help              Print help
"#,
};

pub struct Options {
    rid: Id,
    confirm: bool,
}

impl Args for Options {
    fn from_args(args: Vec<OsString>) -> anyhow::Result<(Self, Vec<OsString>)> {
        use lexopt::prelude::*;

        let mut parser = lexopt::Parser::from_args(args);
        let mut id: Option<Id> = None;
        let mut confirm = true;

        while let Some(arg) = parser.next()? {
            match arg {
                Long("no-confirm") => {
                    confirm = false;
                }
                Long("help") | Short('h') => {
                    return Err(Error::Help.into());
                }
                Value(val) if id.is_none() => {
                    id = Some(term::args::rid(&val)?);
                }
                _ => return Err(anyhow::anyhow!(arg.unexpected())),
            }
        }

        Ok((
            Options {
                rid: id.ok_or_else(|| anyhow!("an RID must be provided; see `rad rm --help`"))?,
                confirm,
            },
            vec![],
        ))
    }
}

pub fn run(options: Options, ctx: impl term::Context) -> anyhow::Result<()> {
    let profile = ctx.profile()?;
    let storage = &profile.storage;
    let rid = options.rid;
    let path = storage::git::paths::repository(storage, &rid);

    if !path.exists() {
        anyhow::bail!("repository {rid} was not found");
    }

    if !options.confirm || term::confirm(format!("Remove {rid}?")) {
        untrack(&rid, &profile)?;
        remove_remote(&rid)?;
        storage.remove(rid)?;
        term::success!("Successfully removed {rid} from storage");
    }

    Ok(())
}

fn untrack(rid: &Id, profile: &Profile) -> anyhow::Result<()> {
    let mut node = radicle::Node::new(profile.socket());

    let result = if node.is_running() {
        node.untrack_repo(*rid).map_err(anyhow::Error::from)
    } else {
        let mut store =
            node::tracking::store::Config::open(profile.home.node().join(node::TRACKING_DB_FILE))?;
        store.untrack_repo(rid).map_err(anyhow::Error::from)
    };

    if let Err(e) = result {
        term::warning(format!("Failed to untrack repository: {e}"));
        term::warning("Make sure to untrack this repository when your node is running");
    } else {
        term::success!("Untracked {rid}")
    }

    Ok(())
}

fn remove_remote(rid: &Id) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    if let Err(e) = git::Repository::open(cwd)
        .map_err(|err| err.into())
        .and_then(|repo| git::remove_remote(&repo, rid))
    {
        term::warning(format!(
            "Attempted to remove 'rad' remote from working copy: {e}"
        ));
        term::warning("In case a working copy exists, make sure to `git remote remove rad`");
    } else {
        term::success!("Successfully removed 'rad' remote");
    }

    Ok(())
}
