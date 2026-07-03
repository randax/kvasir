use tokio::sync::broadcast;

const UPDATE_CHANNEL_CAPACITY: usize = 32;

/// A change to stored data that subscribed viewers should hear about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DataUpdate {
    RollupsChanged,
    ContentImported,
    Cleared,
}

/// The daemon's single change-notification channel. Every mutation path
/// announces through [`UpdateChannel::notify`]; every subscription listens
/// through [`UpdateChannel::subscribe`].
#[derive(Clone)]
pub(crate) struct UpdateChannel {
    sender: broadcast::Sender<DataUpdate>,
}

impl UpdateChannel {
    pub(crate) fn new() -> Self {
        let (sender, _receiver) = broadcast::channel(UPDATE_CHANNEL_CAPACITY);
        Self { sender }
    }

    pub(crate) fn notify(&self, update: DataUpdate) {
        let _ = self.sender.send(update);
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<DataUpdate> {
        self.sender.subscribe()
    }
}
