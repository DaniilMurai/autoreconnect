#[cfg(target_os = "macos")]
pub mod macos;

use crossbeam_channel::Sender;

use crate::core::model::{CredError, LinkEvent, LinkState, NetworkId, ProbeTargets, WifiError};
use crate::core::probe::Prober;

/// No Serialize and a redacted Debug, so the Wi-Fi password cannot reach the event log.
pub struct Secret(String);

impl Secret {
    pub fn new(s: String) -> Self {
        Secret(s)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinMode {
    FromCache,
    FreshScanStrongest,
}

pub trait WifiControl {
    fn link(&self) -> Result<LinkState, WifiError>;
    fn remember_current(&mut self) -> Result<Option<NetworkId>, WifiError>;
    fn join(&mut self, network: &NetworkId, mode: JoinMode, password: Option<&Secret>) -> Result<(), WifiError>;
    fn set_power(&self, on: bool) -> Result<(), WifiError>;
    fn subscribe(&mut self, events: Sender<LinkEvent>) -> Result<(), WifiError>;
    fn probe_targets(&self) -> ProbeTargets;
}

pub trait CredentialStore {
    /// False on Windows: the WLAN profile holds the key, `join` gets `None`.
    fn required(&self) -> bool;
    /// The app's own copy (macOS: generic password in the login keychain).
    fn get(&self, network: &NetworkId) -> Result<Option<Secret>, CredError>;
    /// Reads the OS copy. macOS: System keychain, one admin prompt (S-06). Only at first sight of a network.
    fn import_from_system(&self, network: &NetworkId) -> Result<Secret, CredError>;
    fn put(&self, network: &NetworkId, secret: &Secret) -> Result<(), CredError>;
}

/// Grows a `login` field in T-17.
pub struct Platform {
    pub wifi: Box<dyn WifiControl>,
    pub creds: Box<dyn CredentialStore>,
    pub prober: Box<dyn Prober>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionState {
    NotDetermined,
    Granted,
    Denied,
    NotApplicable,
}

/// Runs on the main thread before the core starts; the core gets the result, not the object.
pub trait Permissions {
    fn location(&self) -> PermissionState;
    /// Shows the system prompt once; the answer arrives on the main run loop.
    fn request_location(&self);
}
