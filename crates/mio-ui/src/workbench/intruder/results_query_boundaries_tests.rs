use gpui_kit::TestAppContext;
use gpui_kit::test::{TestQueryExt, TestWindowExt};
use mio_core::IntruderRunState;

use super::results_query_tests::{answer, click, emit, flow, open, page, query, row, run};
use super::results_view_tests::close_tests::{answer_close_dialog, has_close_dialog};
use super::*;
use crate::workbench::tests::drain;

mod part_1;
mod part_2;
