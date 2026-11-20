use gpui_kit::TestAppContext;
use gpui_kit::test::{TestQueryExt, TestWindowExt};
use mio_core::{ExtCollectionReport, ExtInspection};

use super::super::tests::{Fixture, drain, fixture, project_menu, pump_native, tab};
use super::*;

mod coverage;
mod part_1;
mod part_10;
mod part_2;
mod part_3;
mod part_4;
mod part_5;
mod part_6;
mod part_7;
mod part_8;
mod part_9;

pub(in crate::workbench) use part_1::report;
use part_1::{click, convert, inspect, inspected};
use part_7::{answer_ext_picker, close_ext_picker_receiver, open_ext_picker};
