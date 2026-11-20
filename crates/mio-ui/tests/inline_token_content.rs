extern crate gpui_kit as gpui;

use gpui_kit::component::input::{
    InlineToken, InlineTokenError, InputContent, InputState, Textarea, TextareaState,
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, Context, Entity, EntityInputHandler, IntoElement, Render,
    TestAppContext, Window, WindowBounds, WindowOptions, div, point, prelude::*, px, size,
};

macro_rules! kbd {
    ("shift-z") => {{
        #[cfg(target_os = "macos")]
        {
            "cmd-shift-z"
        }
        #[cfg(not(target_os = "macos"))]
        {
            "ctrl-y"
        }
    }};
    ($rest:literal) => {{
        #[cfg(target_os = "macos")]
        {
            concat!("cmd-", $rest)
        }
        #[cfg(not(target_os = "macos"))]
        {
            concat!("ctrl-", $rest)
        }
    }};
}

struct TokenEditor(Entity<TextareaState>);

impl Render for TokenEditor {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(Textarea::new(&self.0).h_full())
    }
}

fn fixture(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<TextareaState>) {
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(600.), px(400.)),
                })),
                ..WindowOptions::default()
            },
            cx,
            |window, cx| {
                let input = cx.new(|cx| TextareaState::new(window, cx));
                cx.new(|_| TokenEditor(input))
            },
        )
        .unwrap()
    });
    let input = cx.read(|cx| view.read(cx).0.clone());
    (window, input)
}

fn pair(text: &str, id: &str) -> InputContent {
    let value = format!("§{text}§");
    let end = value.len();
    InputContent::new(value)
        .with_token(0..2, InlineToken::new(format!("{id}:start"), "§"))
        .unwrap()
        .with_token(end - 2..end, InlineToken::new(format!("{id}:end"), "§"))
        .unwrap()
}

#[gpui_kit::test]
fn marker_pair_and_inner_typing_have_separate_atomic_undo_steps(cx: &mut TestAppContext) {
    let (window, input) = fixture(cx);
    let original = "literal §: 中国!";
    let start = original.find('中').unwrap();
    cx.update_window(window, |_, window, cx| {
        input.update(cx, |state, cx| {
            state.set_value(original, window, cx);
            state
                .replace_range_with_content(start..start + 6, pair("中国", "position"), window, cx)
                .unwrap();
            state.focus(window, cx);
        });
        window.render_frame(cx);
        assert_eq!(input.read(cx).tokens().len(), 2);
        window.press(kbd!("z"), cx);
        assert_eq!(input.read(cx).value().as_ref(), original);
        assert!(input.read(cx).tokens().is_empty());
        window.press(kbd!("shift-z"), cx);
        assert_eq!(input.read(cx).value().as_ref(), "literal §: §中国§!");
        assert_eq!(input.read(cx).tokens().len(), 2);
        input.update(cx, |state, cx| {
            state.set_selected_range(start + 2..start + 5, cx)
        });
        window.input("海", cx);
        assert_eq!(input.read(cx).value().as_ref(), "literal §: §海国§!");
        assert_eq!(input.read(cx).tokens().len(), 2);
        window.press(kbd!("z"), cx);
        assert_eq!(input.read(cx).value().as_ref(), "literal §: §中国§!");
        window.press(kbd!("z"), cx);
        assert_eq!(input.read(cx).value().as_ref(), original);
        assert!(input.read(cx).tokens().is_empty());
    })
    .unwrap();
}

#[gpui_kit::test]
fn content_replacement_restores_all_token_identities_after_clear_and_overlap(
    cx: &mut TestAppContext,
) {
    let (window, input) = fixture(cx);
    let first = pair("a", "one");
    let second = pair("b", "two");
    let offset = first.text().len() + 3;
    let mut content = InputContent::new(format!("{} / {}", first.text(), second.text()));
    for span in first.tokens() {
        content = content
            .with_token(span.range(), span.token().clone())
            .unwrap();
    }
    for span in second.tokens() {
        let range = span.range();
        content = content
            .with_token(
                range.start + offset..range.end + offset,
                span.token().clone(),
            )
            .unwrap();
    }
    cx.update_window(window, |_, window, cx| {
        input.update(cx, |state, cx| {
            state.set_value("a / b", window, cx);
            state
                .replace_range_with_content(0..5, content.clone(), window, cx)
                .unwrap();
            state.focus(window, cx);
        });
        window.render_frame(cx);
        window.press(kbd!("z"), cx);
        assert_eq!(input.read(cx).value().as_ref(), "a / b");
        assert!(input.read(cx).tokens().is_empty());
        window.press(kbd!("shift-z"), cx);
        assert_eq!(input.read(cx).content(), content);
        input.update(cx, |state, cx| {
            state
                .replace_range_with_content(
                    0..content.text().len(),
                    InputContent::new("a / b"),
                    window,
                    cx,
                )
                .unwrap();
        });
        assert!(input.read(cx).tokens().is_empty());
        window.press(kbd!("z"), cx);
        assert_eq!(input.read(cx).content(), content);
        input.update(cx, |state, cx| {
            state
                .replace_range_with_content(0..0, InputContent::new("🙂"), window, cx)
                .unwrap();
        });
        assert_eq!(input.read(cx).tokens()[0].range(), 4..6);
        window.press(kbd!("z"), cx);
        assert_eq!(input.read(cx).content(), content);

        input.update(cx, |state, cx| {
            state.set_value(
                InputContent::new("P@first!")
                    .with_token(1..7, InlineToken::new("ref", "@first"))
                    .unwrap(),
                window,
                cx,
            );
            state
                .replace_range_with_content(2..3, pair("中", "new"), window, cx)
                .unwrap();
        });
        assert_eq!(input.read(cx).value().as_ref(), "P§中§!");
        window.press(kbd!("z"), cx);
        assert_eq!(input.read(cx).value().as_ref(), "P@first!");
        assert_eq!(input.read(cx).tokens()[0].token().id().as_ref(), "ref");
    })
    .unwrap();
}

#[gpui_kit::test]
fn rejected_content_edits_leave_text_tokens_and_ime_unchanged(cx: &mut TestAppContext) {
    let (window, input) = fixture(cx);
    let content = InputContent::new("中 @a!")
        .with_token(4..6, InlineToken::new("ref", "@a"))
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        input.update(cx, |state, cx| {
            state.set_value(content.clone(), window, cx);
            assert_eq!(
                state.replace_range_with_content(1..1, pair("x", "bad"), window, cx),
                Err(InlineTokenError::InvalidBoundary),
            );
            assert_eq!(
                state.replace_range_with_content(6..6, InputContent::new("\u{301}"), window, cx),
                Err(InlineTokenError::InvalidBoundary),
            );
            assert_eq!(state.content(), content);
            state.replace_and_mark_text_in_range(Some(0..0), "问", Some(1..1), window, cx);
            let composition = state.content();
            assert_eq!(
                state.replace_range_with_content(0..0, pair("x", "ime"), window, cx),
                Err(InlineTokenError::CompositionActive),
            );
            assert_eq!(state.content(), composition);
            state.replace_text_in_range(None, "问候", window, cx);
            state.focus(window, cx);
        });
        window.render_frame(cx);
        window.press(kbd!("z"), cx);
        assert_eq!(input.read(cx).content(), content);
    })
    .unwrap();
}

#[gpui_kit::test]
fn token_only_edits_advance_revision_and_keep_native_history(cx: &mut TestAppContext) {
    let (window, input) = fixture(cx);
    cx.update_window(window, |_, window, cx| {
        let first = pair("a", "first");
        let second = pair("a", "second");
        input.update(cx, |state, cx| {
            state.set_value(first.clone(), window, cx);
            state.focus(window, cx);
        });
        window.render_frame(cx);
        let before = input.read(cx).document_revision();
        input.update(cx, |state, cx| {
            state
                .replace_range_with_content(0..first.text().len(), second.clone(), window, cx)
                .unwrap();
        });
        let replaced = input.read(cx).document_revision();
        assert_ne!(before, replaced, "token identity edits are synchronous");
        assert_eq!(input.read(cx).content(), second);
        input.update(cx, |state, cx| {
            state
                .replace_range_with_content(0..second.text().len(), second.clone(), window, cx)
                .unwrap();
            state.set_selected_range(2..3, cx);
        });
        assert_eq!(input.read(cx).document_revision(), replaced);
        window.press(kbd!("z"), cx);
        assert_eq!(input.read(cx).content(), first);
        let undone = input.read(cx).document_revision();
        assert_ne!(undone, replaced);
        window.press(kbd!("shift-z"), cx);
        assert_eq!(input.read(cx).content(), second);
        assert_ne!(input.read(cx).document_revision(), undone);
    })
    .unwrap();
}

#[gpui_kit::test]
fn single_line_content_keeps_mask_and_normalization_guards(cx: &mut TestAppContext) {
    let (window, _) = fixture(cx);
    cx.update_window(window, |_, window, cx| {
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            let original = pair("a", "one");
            state.set_value(original.clone(), window, cx);
            state.set_masked(true, window, cx);
            assert_eq!(
                state.replace_range_with_content(0..0, pair("x", "masked"), window, cx),
                Err(InlineTokenError::UnsupportedMode),
            );
            assert_eq!(state.content(), original);
            state.set_masked(false, window, cx);
            assert_eq!(
                state.replace_range_with_content(0..0, InputContent::new("a\nb"), window, cx),
                Err(InlineTokenError::TextMismatch),
            );
            assert_eq!(state.content(), original);
        });
    })
    .unwrap();
}
