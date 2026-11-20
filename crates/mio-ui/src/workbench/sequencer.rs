use gpui_kit::accesskit::Role;
use gpui_kit::component::{
    button::ButtonVariants,
    input::{Editor, EditorState, Input, InputEvent, InputState},
};
use gpui_kit::{AnyElement, Context, Entity, FontWeight, Subscription, Window};
use mio_core::{
    Command, DEFAULT_SEQUENCER_CONCURRENCY, DEFAULT_SEQUENCER_TARGET_SAMPLES, Draft, HttpVersion,
    MAX_SEQUENCER_CONCURRENCY, SequencerCaptureConfig, SequencerCommand, SequencerEvent, Uuid,
    sequencer::{
        FipsStatus, FipsTest, MAX_SAMPLE_COUNT, MAX_TOKEN_BYTES, MAX_TOTAL_SAMPLE_BYTES,
        MIN_FIPS_TEST_BITS, MIN_SAMPLE_COUNT, MIN_TOKEN_BYTES, QualityRating, SampleEncoding,
        SequencerAnalysis, SequencerTokenLocation,
    },
};
use tokio_util::sync::CancellationToken;

use super::*;

mod part_01;
mod part_02;
mod part_03;
mod part_04;
mod part_05;
mod part_06;

pub(in crate::workbench) use part_01::*;
pub(in crate::workbench) use part_03::*;
