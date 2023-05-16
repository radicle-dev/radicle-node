pub mod fetch;
pub use fetch::fetch;

pub mod ls_refs;
pub use ls_refs::ls_refs;

pub use gix_transport::client::Connection;

pub fn connection(repo: BString, read: R, write: W) -> Connection {
    // TODO: not sure if this is correct
    let url = format!("heartwood://{}", repo);

    // TODO: do we actually have a virtual host?
    Connection::new(
        read,
        write,
        Protocol::V2,
        repo,
        None::<(String, Option<u16>)>,
        ConnectMode::Daemon,
    )
    .custom_url(Some(url))
}
