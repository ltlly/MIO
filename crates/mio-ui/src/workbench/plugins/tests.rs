use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::{TestAppContext, test::TestWindowExt};
use mio_core::{
    PluginAction, PluginOutcome, PluginWorkspaceEdit,
    panel::{Editor, EditorFormat, Node, PanelSnapshot, node, ui_event},
    plugins::PluginManifest,
    protocol::ChannelIdentity,
};

use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, pump_native, tab};

mod commit_quit;
mod commit_workspace;
mod extensions_suite;
mod helpers;
mod settings_inflight;
mod settings_rebind_preserve;
mod settings_rebind_send;
mod settings_save_as;
mod settings_save_queue;
mod v13_features;
mod v13_intercept_response;

use helpers::*;
