//! Private Hyprland JSON wire representations.

use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::super::BackendError;

#[derive(Deserialize)]
pub(super) struct Workspace {
    pub(super) id: i64,
}

#[derive(Deserialize)]
pub(super) struct ClientWorkspace {
    pub(super) id: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Client {
    pub(super) stable_id: String,
    pub(super) address: String,
    pub(super) workspace: ClientWorkspace,
    pub(super) floating: bool,
    pub(super) fullscreen: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ActiveWindow {
    pub(super) stable_id: String,
    pub(super) address: String,
}

pub(super) fn parse_workspaces(json: &str) -> Result<Vec<Workspace>, BackendError> {
    parse(json)
}

pub(super) fn parse_clients(json: &str) -> Result<Vec<Client>, BackendError> {
    parse(json)
}

pub(super) fn parse_active_window(json: &str) -> Result<ActiveWindow, BackendError> {
    parse(json)
}

fn parse<T: DeserializeOwned>(json: &str) -> Result<T, BackendError> {
    serde_json::from_str(json).map_err(|error| {
        if error.is_syntax() || error.is_eof() {
            BackendError::ObservedStateUnavailable
        } else {
            BackendError::InconsistentObservedState
        }
    })
}
