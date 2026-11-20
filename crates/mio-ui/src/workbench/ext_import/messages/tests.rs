use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{KeyBinding, TestAppContext, point};

use super::super::super::tests::{Fixture, drain, fixture, project_menu, pump_native, tab};
use super::super::tests::report;
use super::*;

mod part_1;
mod part_2;
mod part_3;
mod part_4;
mod part_5;
mod part_6;
mod part_7;
mod part_8;
mod part_9;

use part_1::{choose_collection, open, page, query, select};
pub(in crate::workbench) use part_1::{click, fragment, open_report, read, summary};
use part_3::archive_quit_fixture;
