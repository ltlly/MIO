use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{AnyWindowHandle, ScrollDelta, TestAppContext, point};
use mio_core::{
    IntruderResultOutcome, IntruderResultPage, IntruderResultSummary, IntruderResultViewPage,
    IntruderResultViewQuery, IntruderResultViewRow, IntruderRunState, TrafficSource,
};

use super::tests::{attack, intruder_fixture};
use super::*;
use crate::workbench::intruder::results_view_tests::close_tests::{
    answer_close_dialog, has_close_dialog,
};
use crate::workbench::tests::{Fixture, drain, record};

#[path = "results_baseline_tests.rs"]
mod baseline_tests;
#[path = "results_close_tests.rs"]
pub(super) mod close_tests;
mod part_1;
mod part_2;
mod part_3;
mod part_4;
mod part_5;
mod part_6;
mod part_7;
#[path = "results_pool_tests.rs"]
mod pool_tests;
#[path = "results_running_save_tests.rs"]
mod running_save_tests;
pub(in crate::workbench::intruder) use part_1::*;
pub(in crate::workbench::intruder) use part_2::*;
