use std::ffi::OsString;
use std::path::Path;
use std::time;

use anyhow::{anyhow, Context as _};

use radicle::node;
use radicle::node::{FetchResult, FetchResults, Handle as _, Node};
use radicle::prelude::{Id, NodeId};

use crate::terminal as term;
use crate::terminal::args::{Args, Error, Help};

pub const HELP: Help = Help {
    name: "sync",
    description: "Sync repositories to the network",
    version: env!("CARGO_PKG_VERSION"),
    usage: r#"
Usage

    rad sync [<rid>] [<option>...]
    rad sync [<rid>] [--fetch] [<rid>] [<option>...]
    rad sync [<rid>] [--announce] [<rid>] [<option>...]

    By default, the current repository is synchronized both ways.
    If an <rid> is specified, that repository is synced instead.

    The process begins by fetching changes from connected seeds,
    followed by announcing local refs to peers, thereby prompting
    them to fetch from us.

    When `--fetch` is specified, any number of seeds may be given
    using the `--seed` option, eg. `--seed <nid>@<addr>:<port>`.

    When `--replicas` is specified, the given replication factor will try
    to be matched. For example, `--replicas 5` will sync with 5 seeds.

    When `--fetch` or `--announce` are specified on their own, this command
    will only fetch or announce.

Options

    --fetch, -f               Turn on fetching (default: true)
    --announce, -a            Turn on announcing (default: true)
    --timeout <secs>          How many seconds to wait while syncing
    --seed <nid>              Sync with the given node (may be specified multiple times)
    --replicas, -r <count>    Sync with a specific number of seeds
    --verbose, -v             Verbose output
    --help                    Print help
"#,
};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncOptions {
    mode: SyncMode,
    direction: SyncDirection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncMode {
    Replicas(usize),
    Seeds(Vec<NodeId>),
}

impl Default for SyncMode {
    fn default() -> Self {
        Self::Replicas(3)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum SyncDirection {
    Fetch,
    Announce,
    #[default]
    Both,
}

#[derive(Default, Debug)]
pub struct Options {
    pub rid: Option<Id>,
    pub verbose: bool,
    pub timeout: time::Duration,
    pub sync: SyncOptions,
}

impl Args for Options {
    fn from_args(args: Vec<OsString>) -> anyhow::Result<(Self, Vec<OsString>)> {
        use lexopt::prelude::*;

        let mut parser = lexopt::Parser::from_args(args);
        let mut verbose = false;
        let mut timeout = time::Duration::from_secs(9);
        let mut rid = None;
        let mut sync = SyncOptions::default();

        while let Some(arg) = parser.next()? {
            match arg {
                Long("verbose") | Short('v') => {
                    verbose = true;
                }
                Long("fetch") | Short('f') => {
                    sync.direction = match sync.direction {
                        SyncDirection::Both => SyncDirection::Fetch,
                        SyncDirection::Announce => SyncDirection::Both,
                        SyncDirection::Fetch => SyncDirection::Fetch,
                    };
                }
                Long("replicas") | Short('r') => {
                    let val = parser.value()?;
                    let count = term::args::number(&val)?;

                    if let SyncMode::Replicas(ref mut r) = sync.mode {
                        *r = count;
                    } else {
                        anyhow::bail!("`--replicas` (-r) cannot be specified with `--seed`");
                    }
                }
                Long("seed") => {
                    let val = parser.value()?;
                    let nid = term::args::nid(&val)?;

                    if let SyncMode::Seeds(ref mut seeds) = sync.mode {
                        seeds.push(nid);
                    } else {
                        sync.mode = SyncMode::Seeds(vec![nid]);
                    }
                }
                Long("announce") | Short('a') => {
                    sync.direction = match sync.direction {
                        SyncDirection::Both => SyncDirection::Announce,
                        SyncDirection::Announce => SyncDirection::Announce,
                        SyncDirection::Fetch => SyncDirection::Both,
                    };
                }
                Long("timeout") | Short('t') => {
                    let value = parser.value()?;
                    let secs = term::args::parse_value("timeout", value)?;

                    timeout = time::Duration::from_secs(secs);
                }
                Long("help") | Short('h') => {
                    return Err(Error::Help.into());
                }
                Value(val) if rid.is_none() => {
                    rid = Some(term::args::rid(&val)?);
                }
                arg => {
                    return Err(anyhow!(arg.unexpected()));
                }
            }
        }

        if sync.direction == SyncDirection::Announce {
            if let SyncMode::Seeds(_) = sync.mode {
                anyhow::bail!("`--seed` is only supported when fetching.");
            }
        }

        Ok((
            Options {
                rid,
                verbose,
                timeout,
                sync,
            },
            vec![],
        ))
    }
}

pub fn run(options: Options, ctx: impl term::Context) -> anyhow::Result<()> {
    let profile = ctx.profile()?;
    let rid = match options.rid {
        Some(rid) => rid,
        None => {
            let (_, rid) = radicle::rad::repo(Path::new("."))
                .context("Current directory is not a radicle project")?;

            rid
        }
    };
    let mut node = radicle::Node::new(profile.socket());
    let mode = options.sync.mode;

    if [SyncDirection::Fetch, SyncDirection::Both].contains(&options.sync.direction) {
        if !profile.tracking()?.is_repo_tracked(&rid)? {
            anyhow::bail!("repository {rid} is not tracked");
        }
        let results = fetch(rid, mode.clone(), options.timeout, &mut node)?;
        let success = results.success().count();
        let failed = results.failed().count();

        if success == 0 {
            term::error(format!("Failed to fetch repository from {failed} seed(s)"));
        } else {
            term::success!("Fetched repository from {success} seed(s)");
        }
    }
    if [SyncDirection::Announce, SyncDirection::Both].contains(&options.sync.direction) {
        announce(rid, mode, options.timeout, node)?;
    }
    Ok(())
}

fn announce(
    rid: Id,
    _mode: SyncMode,
    timeout: time::Duration,
    mut node: Node,
) -> anyhow::Result<()> {
    let seeds = node.seeds(rid)?;
    let connected = seeds.connected().map(|s| s.nid).collect::<Vec<_>>();
    if connected.is_empty() {
        term::info!("Not connected to any seeds.");
        return Ok(());
    }

    let mut spinner = term::spinner(format!("Syncing with {} node(s)..", connected.len()));
    let result = node.announce(rid, connected, timeout, |event| match event {
        node::AnnounceEvent::Announced => {}
        node::AnnounceEvent::RefsSynced { remote } => {
            spinner.message(format!("Synced with {remote}.."));
        }
    })?;

    if result.synced.is_empty() {
        spinner.failed();
    } else {
        spinner.message(format!("Synced with {} node(s)", result.synced.len()));
        spinner.finish();
    }
    for seed in result.timeout {
        term::notice!("Seed {seed} timed out..");
    }
    if result.synced.is_empty() {
        anyhow::bail!("all seeds timed out");
    }
    Ok(())
}

pub fn fetch(
    rid: Id,
    mode: SyncMode,
    timeout: time::Duration,
    node: &mut Node,
) -> Result<FetchResults, node::Error> {
    match mode {
        SyncMode::Seeds(seeds) => {
            let mut results = FetchResults::default();
            for seed in seeds {
                let result = fetch_from(rid, &seed, node)?;
                results.push(seed, result);
            }
            Ok(results)
        }
        SyncMode::Replicas(count) => fetch_all(rid, count, timeout, node),
    }
}

fn fetch_all(
    rid: Id,
    count: usize,
    timeout: time::Duration,
    node: &mut Node,
) -> Result<FetchResults, node::Error> {
    // Get seeds. This consults the local routing table only.
    let seeds = node.seeds(rid)?;
    let mut results = FetchResults::default();
    let (mut connected, mut disconnected) = seeds.partition();

    // N.b. sort the seeds for deterministic testing.
    connected.sort_by(|x, y| x.nid.cmp(&y.nid));
    disconnected.sort_by(|x, y| x.nid.cmp(&y.nid));

    // Fetch from connected seeds.
    for seed in connected.iter().take(count) {
        let result = fetch_from(rid, &seed.nid, node)?;
        results.push(seed.nid, result);
    }

    // Try to connect to disconnected seeds and fetch from them.
    while results.success().count() < count {
        let Some(seed) = disconnected.pop() else {
            break;
        };
        // Try all seed addresses until one succeeds.
        for ka in seed.addrs {
            let spinner = term::spinner(format!(
                "Connecting to {}@{}..",
                term::format::tertiary(&seed.nid),
                term::format::tertiary(&ka.addr)
            ));
            let cr = node.connect(
                seed.nid,
                ka.addr,
                node::ConnectOptions {
                    persistent: false,
                    timeout,
                },
            )?;

            match cr {
                node::ConnectResult::Connected => {
                    spinner.finish();
                    let result = fetch_from(rid, &seed.nid, node)?;
                    results.push(seed.nid, result);
                    break;
                }
                node::ConnectResult::Disconnected { .. } => {
                    spinner.failed();
                    continue;
                }
            }
        }
    }

    Ok(results)
}

fn fetch_from(rid: Id, seed: &NodeId, node: &mut Node) -> Result<FetchResult, node::Error> {
    let spinner = term::spinner(format!(
        "Fetching {} from {}..",
        term::format::tertiary(rid),
        term::format::tertiary(term::format::node(seed))
    ));
    let result = node.fetch(rid, *seed)?;

    match &result {
        FetchResult::Success { .. } => {
            spinner.finish();
        }
        FetchResult::Failed { reason } => {
            spinner.error(reason);
        }
    }
    Ok(result)
}
