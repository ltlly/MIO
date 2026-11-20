use super::*;
use crate::workbench::tests::{Fixture, drain, fixture};
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{MouseButton, TestAppContext};
use mio_core::{ExtHttpHighlight, ExtItemSummary};

mod part_1;
mod part_2;
mod part_3;
mod part_4;
mod part_5;
mod part_6;
mod part_7;
mod part_8;

use part_1::{ack, click, input, load, open, setup, write};
use part_5::sidebar;
use part_6::load_common;
