use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{
    AnyWindowHandle, Bounds, ElementInputHandler, EntityInputHandler,
    InputEvent as PlatformInputEvent, InputHandler, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Point, TestAppContext, WindowBounds, WindowOptions, point, size,
};
use mio_core::{Draft, FlowState, FlowSummary, TrafficSource, Uuid};

use super::*;

mod history;
mod main_cases_1;
mod main_cases_10;
mod main_cases_11;
mod main_cases_2;
mod main_cases_3;
mod main_cases_4;
mod main_cases_5;
mod main_cases_6;
mod main_cases_7;
mod main_cases_8;
mod main_cases_9;
mod main_helpers;
mod proxy_stop;
mod repeater_cancel;
pub(super) mod settings;
#[cfg(target_os = "macos")]
mod shutdown;
mod user_preferences;

#[allow(unused_imports)]
pub(crate) use main_helpers::*;
