use crate::core::events::Event;
use crate::core::model::Health;

#[derive(Clone, Debug, PartialEq)]
pub struct UiModel {
    pub health: Health,
    pub paused: bool,
    pub network: Option<String>,
    /// The last 5 `recovery_action` events, newest last (AC-8).
    pub recent: Vec<Event>,
    pub start_at_login: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiCommand {
    Pause,
    Resume,
    SetStartAtLogin(bool),
    RetryCredential,
    Quit,
}
