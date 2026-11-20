use mio_core::{IntruderAutoThrottle, IntruderPoolInputs};

use super::*;

pub(in crate::workbench) use mio_core::IntruderPoolDelayMode as DelayKind;

pub(in crate::workbench) struct CreatePoolForm {
    pub(in crate::workbench) id: Uuid,
    pub(in crate::workbench) existing: Option<Uuid>,
    pub(in crate::workbench) creating: bool,
    pub(in crate::workbench) name: Entity<InputState>,
    pub(in crate::workbench) cap: Entity<InputState>,
    pub(in crate::workbench) delay: Entity<InputState>,
    pub(in crate::workbench) increment: Entity<InputState>,
    pub(in crate::workbench) other: Entity<InputState>,
    pub(in crate::workbench) cap_enabled: bool,
    pub(in crate::workbench) delay_enabled: bool,
    pub(in crate::workbench) delay_kind: DelayKind,
    pub(in crate::workbench) automatic: bool,
    pub(in crate::workbench) status_429: bool,
    pub(in crate::workbench) status_503: bool,
    pub(in crate::workbench) other_enabled: bool,
    pub(in crate::workbench) error: Option<String>,
    initial_inputs: (IntruderPoolInputs, bool, bool),
    had_retained_inputs: bool,
}

impl CreatePoolForm {
    pub(in crate::workbench) fn new(
        selection: Option<&IntruderPoolSelection>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let pool = match selection {
            Some(IntruderPoolSelection::Create(pool)) => Some(pool),
            _ => None,
        };
        let existing = match selection {
            Some(IntruderPoolSelection::Existing(id)) => Some(*id),
            _ => None,
        };
        let (delay_kind, delay, increment) = match pool.and_then(|pool| pool.delay) {
            None => (DelayKind::Fixed, String::new(), String::new()),
            Some(IntruderPoolDelay::Fixed { milliseconds }) => {
                (DelayKind::Fixed, milliseconds.to_string(), String::new())
            }
            Some(IntruderPoolDelay::Random { milliseconds }) => {
                (DelayKind::Random, milliseconds.to_string(), String::new())
            }
            Some(IntruderPoolDelay::Increment {
                initial_ms,
                increment_ms,
            }) => (
                DelayKind::Increment,
                initial_ms.to_string(),
                increment_ms.to_string(),
            ),
        };
        let auto = pool
            .map(|pool| pool.automatic_throttling.clone())
            .unwrap_or_default();
        let retained = pool.and_then(|pool| pool.retained_inputs.clone());
        let had_retained_inputs = retained.is_some();
        let inputs = retained.unwrap_or_else(|| IntruderPoolInputs {
            cap: pool
                .and_then(|pool| pool.max_concurrent_requests)
                .map(|cap| cap.to_string())
                .unwrap_or_default(),
            delay,
            increment,
            delay_mode: delay_kind,
        });
        let cap_enabled = pool.is_some_and(|pool| pool.max_concurrent_requests.is_some());
        let delay_enabled = pool.is_some_and(|pool| pool.delay.is_some());
        let initial_inputs = (inputs.clone(), cap_enabled, delay_enabled);
        let mut input =
            |value: String| cx.new(|cx| InputState::new(window, cx).default_value(value));
        Self {
            id: pool.map_or_else(Uuid::new_v4, |pool| pool.id),
            existing,
            creating: pool.is_some(),
            name: input(pool.map(|pool| pool.name.clone()).unwrap_or_default()),
            cap: input(inputs.cap),
            delay: input(inputs.delay),
            increment: input(inputs.increment),
            other: input(auto.other_codes),
            cap_enabled,
            delay_enabled,
            delay_kind: inputs.delay_mode,
            automatic: auto.enabled,
            status_429: auto.status_429,
            status_503: auto.status_503,
            other_enabled: auto.other_enabled,
            error: None,
            initial_inputs,
            had_retained_inputs,
        }
    }

    pub(in crate::workbench) fn inputs(&self) -> [Entity<InputState>; 5] {
        [
            self.name.clone(),
            self.cap.clone(),
            self.delay.clone(),
            self.increment.clone(),
            self.other.clone(),
        ]
    }

    pub(in crate::workbench) fn has_composition(&self, window: &mut Window, cx: &mut App) -> bool {
        self.inputs().iter().any(|input| {
            input.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            })
        })
    }

    pub(in crate::workbench) fn snapshot(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<IntruderResourcePool, String> {
        self.snapshot_inputs(false, window, cx)
    }

    pub(in crate::workbench) fn snapshot_preserving_inputs(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<IntruderResourcePool, String> {
        self.snapshot_inputs(true, window, cx)
    }

    pub(in crate::workbench) fn accept_retained_inputs(&mut self) {
        self.had_retained_inputs = true;
    }

    fn snapshot_inputs(
        &self,
        force: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<IntruderResourcePool, String> {
        let [name, cap, delay, increment, other] = self.inputs().map(|input| {
            editing::input_snapshot(&input, window, cx)
                .map(|text| text.to_string())
                .map_err(str::to_owned)
        });
        let (name, cap, delay, increment, other) = (name?, cap?, delay?, increment?, other?);
        let retained = IntruderPoolInputs {
            cap: cap.clone(),
            delay: delay.clone(),
            increment: increment.clone(),
            delay_mode: self.delay_kind,
        };
        let retained_inputs = (force
            || self.had_retained_inputs
            || self.initial_inputs != (retained.clone(), self.cap_enabled, self.delay_enabled))
            .then_some(retained);
        let max_concurrent_requests = self
            .cap_enabled
            .then(|| {
                cap.parse::<u16>()
                    .map_err(|_| "最大并发请求数须为整数。".to_owned())
            })
            .transpose()?;
        let delay = if self.delay_enabled {
            let milliseconds = delay
                .parse::<u64>()
                .map_err(|_| "请求间隔须为非负整数毫秒。".to_owned())?;
            Some(match self.delay_kind {
                DelayKind::Fixed => IntruderPoolDelay::Fixed { milliseconds },
                DelayKind::Random => IntruderPoolDelay::Random { milliseconds },
                DelayKind::Increment => IntruderPoolDelay::Increment {
                    initial_ms: milliseconds,
                    increment_ms: increment
                        .parse::<u64>()
                        .map_err(|_| "间隔增量须为整数毫秒。".to_owned())?,
                },
            })
        } else {
            None
        };
        Ok(IntruderResourcePool {
            id: self.id,
            name,
            max_concurrent_requests,
            delay,
            retained_inputs,
            automatic_throttling: IntruderAutoThrottle {
                enabled: self.automatic,
                status_429: self.status_429,
                status_503: self.status_503,
                other_enabled: self.other_enabled,
                other_codes: other,
            },
        })
    }
}
