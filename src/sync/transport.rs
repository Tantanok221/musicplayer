use sonora_sync_protocol::{ClientMessage, ServerMessage};
use std::future::Future;

pub(crate) trait SyncTransport {
    type Link: SyncLink;

    fn connect(&self) -> impl Future<Output = Option<(Self::Link, ServerMessage)>> + Send;
}

pub(crate) trait SyncLink {
    fn send(&mut self, message: ClientMessage) -> impl Future<Output = Option<()>> + Send;

    fn receive(&mut self) -> impl Future<Output = Option<Frame>> + Send;
}

pub(crate) enum Frame {
    Data(ServerMessage),
    Control,
}
