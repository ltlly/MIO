use std::{fs, path::Path, sync::Arc, task::Poll, time::Duration};

use mio_core::plugins::{
    NativeTrust, PluginEvent, PluginSupervisor, ServiceOperation, ServiceRequest, ServiceResult,
    VerifiedPackage,
};
use mio_plugin_api::{
    FlowRecord, FlowState, FlowSummary, Header, HttpRequest, HttpVersion, ProjectKey,
    TrafficSource, Uuid,
    panel::{
        EditorSnapshot, Empty, Node, PanelSnapshot, PointerEvent, PointerPhase, TableSelection,
        UiEvent, node, ui_event,
    },
    protocol::{CommitEditor, ErrorCode, Permission},
};
use tempfile::TempDir;
use tokio::{net::TcpListener, sync::mpsc, time::timeout};
use tokio_util::sync::CancellationToken;

const QUICK: Duration = Duration::from_secs(3);

#[path = "worker/cases_flow.rs"]
mod cases_flow;
#[path = "worker/cases_lifecycle.rs"]
mod cases_lifecycle;
#[path = "worker/cases_limits.rs"]
mod cases_limits;
#[path = "worker/cases_resync.rs"]
mod cases_resync;
#[path = "worker/helpers.rs"]
mod helpers;

use helpers::*;
