//! Product-command integration: no direct supervisor or fake host-service implementation.
use std::{collections::HashMap, fs, sync::Arc, time::Duration};

use mio_core::{
    Client, Command, Draft, Event, FlowState, LoadedPlugin, PluginAction, PluginOutcome,
    PluginWorkspaceEdit, ProjectKey, Runtime, TrafficSource, Uuid, Workspace,
    panel::{self, EditorSnapshot, PanelSnapshot, UiEvent, node, ui_event},
    protocol::Permission,
};
use tempfile::TempDir;
use tokio::{net::TcpListener, sync::oneshot, time::timeout};

const BOUND: Duration = Duration::from_secs(4);

#[path = "application/cases_hooks.rs"]
mod cases_hooks;
#[path = "application/cases_workspace.rs"]
mod cases_workspace;
#[path = "application/helpers.rs"]
mod helpers;

use helpers::*;
