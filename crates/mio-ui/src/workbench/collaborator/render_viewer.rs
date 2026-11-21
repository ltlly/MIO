use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::{ButtonRounded, ButtonVariants},
    input::{Copy, EditorState},
    native_menu::NativeMenu,
};

use super::*;

#[derive(Clone, Copy, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
enum CollaboratorRawMenuAction {
    SendToRepeater,
    SendToIntruder,
    SendToComparer,
    SendToDecoder,
    CopyCurl,
    Save,
}

fn collaborator_raw_editor_context_menu(
    input: Entity<EditorState>,
    can_operate: bool,
    has_selected: bool,
    is_http: bool,
    raw_empty: bool,
    file_picker_pending: bool,
) -> impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static {
    move |menu, window, cx| {
        input.update(cx, |state, cx| state.focus(window, cx));
        let selected = !input.read(cx).selected_range().is_empty();
        let mut menu = menu
            .menu_with_disabled(
                "发送到 Repeater",
                !can_operate || !is_http || raw_empty,
                Box::new(CollaboratorRawMenuAction::SendToRepeater),
            )
            .menu_with_disabled(
                "发送到 Intruder",
                !can_operate || !is_http || raw_empty,
                Box::new(CollaboratorRawMenuAction::SendToIntruder),
            )
            .menu_with_disabled(
                "发送到 Comparer",
                !has_selected || raw_empty,
                Box::new(CollaboratorRawMenuAction::SendToComparer),
            )
            .menu_with_disabled(
                "发送到 Decoder",
                !has_selected || raw_empty,
                Box::new(CollaboratorRawMenuAction::SendToDecoder),
            )
            .separator()
            .menu_with_disabled("复制", !selected, Box::new(Copy));
        if is_http {
            menu = menu.menu_with_disabled(
                "复制为 curl 命令 (bash)",
                raw_empty,
                Box::new(CollaboratorRawMenuAction::CopyCurl),
            );
        }
        menu.separator().menu_with_disabled(
            "保存…",
            !can_operate || !has_selected || file_picker_pending,
            Box::new(CollaboratorRawMenuAction::Save),
        )
    }
}

fn collaborator_view_tab(
    id: &'static str,
    label: &'static str,
    width: f32,
    selected: bool,
) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .rounded(ButtonRounded::None)
        .relative()
        .self_start()
        .w(px(width))
        .h(px(37.))
        .p_0()
        .bg(theme::surface())
        .text_color(if selected {
            if theme::is_dark() {
                theme::foreground()
            } else {
                rgb(0x000000).into()
            }
        } else if theme::is_dark() {
            theme::muted_foreground()
        } else {
            rgb(0x1f1f1f).into()
        })
        .selected(selected)
        .toggled(selected)
        .accessibility_label(label)
        .child(
            div()
                .text_size(px(13.))
                .font_weight(if selected {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                })
                .child(label),
        )
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(3.))
                    .bg(rgb(0xdb612f)),
            )
        })
}

impl CollaboratorView {
    pub(super) fn render_raw_viewer(
        &mut self,
        wb_entity: &Entity<Workbench>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let (
            can_operate,
            file_picker_pending,
            pretty,
            view,
            has_selected,
            is_http,
            raw_empty,
            meta,
            error,
            editor_elem,
        ) = {
            let wb = wb_entity.read(cx);
            let can_operate = wb.collaborator_can_operate();
            let file_picker_pending = wb.file_picker_pending();
            let pane = &wb.collaborator_pane;
            let pretty = pane.raw_editor.pretty_active();
            let view = pane.raw_editor.view();
            let selected = pane
                .selected_interaction
                .and_then(|id| pane.status.interactions.iter().find(|item| item.id == id))
                .cloned();
            let has_selected = selected.is_some();
            let is_http = selected
                .as_ref()
                .is_some_and(|item| item.protocol == OastProtocol::Http);
            let raw_empty = selected
                .as_ref()
                .is_none_or(|item| item.raw_request.is_empty());
            let meta = selected.as_ref().map_or_else(
                || "选择上方交互记录以查看原始请求报文。".to_owned(),
                |interaction| {
                    let token = interaction.token.as_deref().unwrap_or("未匹配 Token");
                    let label = pane
                        .token_label(interaction.token.as_deref().unwrap_or_default())
                        .unwrap_or_default();
                    let token_part = if label.is_empty() {
                        token.to_owned()
                    } else {
                        format!("{token} ({label})")
                    };
                    let summary = if interaction.protocol == OastProtocol::Dns {
                        format!(
                            "DNS UDP 原始报文 · {} · {} B · 详见 Raw / Hex",
                            interaction.summary,
                            interaction.raw_request.len()
                        )
                    } else {
                        format!(
                            "{} · {} B",
                            interaction.summary,
                            interaction.raw_request.len()
                        )
                    };
                    format!("{token_part}  ·  {summary}  ·  来源 {}", interaction.peer_addr)
                },
            );
            let error = pane.error.clone().unwrap_or_default();
            let editor_elem = editing::message_editor(&pane.raw_editor)
                .readonly(true)
                .size_full()
                .context_menu(collaborator_raw_editor_context_menu(
                    pane.raw_editor.state(),
                    can_operate,
                    has_selected,
                    is_http,
                    raw_empty,
                    file_picker_pending,
                ))
                .aria_label("Collaborator 原始请求")
                .into_any_element();
            (
                can_operate,
                file_picker_pending,
                pretty,
                view,
                has_selected,
                is_http,
                raw_empty,
                meta,
                error,
                editor_elem,
            )
        };
        let _ = file_picker_pending;
        let can_send_repeater = can_operate && is_http && !raw_empty;
        let search_bar = wb_entity.update(cx, |wb, cx| {
            wb.collaborator_pane.raw_editor.render_search_bar(
                "collaborator-raw-editor",
                cx,
                |this, action, window, cx| {
                    this.handle_collaborator_search_action(action, window, cx);
                },
            )
        });

        div()
            .id("collaborator-raw-viewer")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .on_action(
                cx.listener(|this, action: &CollaboratorRawMenuAction, window, cx| {
                    cx.stop_propagation();
                    let _ = this.workbench.update(cx, |wb, cx| match action {
                        CollaboratorRawMenuAction::SendToRepeater => {
                            wb.send_collaborator_interaction_to_repeater(None, window, cx)
                        }
                        CollaboratorRawMenuAction::SendToIntruder => {
                            wb.send_collaborator_interaction_to_intruder(None, window, cx)
                        }
                        CollaboratorRawMenuAction::SendToComparer => {
                            wb.send_collaborator_interaction_to_comparer(None, window, cx)
                        }
                        CollaboratorRawMenuAction::SendToDecoder => {
                            wb.send_collaborator_interaction_to_decoder(None, window, cx)
                        }
                        CollaboratorRawMenuAction::CopyCurl => {
                            wb.copy_collaborator_interaction_curl(None, cx)
                        }
                        CollaboratorRawMenuAction::Save => {
                            wb.save_collaborator_interaction_details(None, window, cx)
                        }
                    });
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w_full()
                    .h(px(38.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .bg(theme::surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .pl(px(10.))
                            .pr(px(12.))
                            .text_size(theme::scaled_px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if dark {
                                theme::foreground()
                            } else {
                                rgb(0x000000).into()
                            })
                            .child("Request"),
                    )
                    .child(
                        collaborator_view_tab(
                            "collaborator-raw-editor-pretty",
                            "Pretty",
                            64.,
                            pretty,
                        )
                        .tooltip("格式化预览交互请求报文")
                        .on_click(cx.listener(|this, _, window, cx| {
                            let _ = this.workbench.update(cx, |wb, cx| {
                                wb.switch_collaborator_raw_pretty(window, cx)
                            });
                            cx.notify();
                        })),
                    )
                    .child(
                        collaborator_view_tab(
                            "collaborator-raw-editor-raw",
                            "Raw",
                            56.,
                            !pretty && view == MessageView::Raw,
                        )
                        .tooltip("查看交互请求原始文本")
                        .on_click(cx.listener(|this, _, window, cx| {
                            let _ = this.workbench.update(cx, |wb, cx| {
                                wb.switch_collaborator_raw_view(MessageView::Raw, window, cx)
                            });
                            cx.notify();
                        })),
                    )
                    .child(
                        collaborator_view_tab(
                            "collaborator-raw-editor-hex",
                            "Hex",
                            53.,
                            !pretty && view == MessageView::Hex,
                        )
                        .tooltip("以十六进制视图查看交互原始字节")
                        .on_click(cx.listener(|this, _, window, cx| {
                            let _ = this.workbench.update(cx, |wb, cx| {
                                wb.switch_collaborator_raw_view(MessageView::Hex, window, cx)
                            });
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .pr(px(10.))
                            .flex()
                            .items_center()
                            .child(
                                button("collaborator-to-repeater", "发送到 Repeater")
                                    .icon(IconName::Repeat)
                                    .tooltip(
                                        "将当前选中的 HTTP 回调原始请求发送到 Repeater 重放分析",
                                    )
                                    .disabled(!can_send_repeater)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let _ = this.workbench.update(cx, |wb, cx| {
                                            wb.send_collaborator_to_repeater(window, cx)
                                        });
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .id("collaborator-interaction-meta")
                    .test_support()
                    .w_full()
                    .flex_shrink_0()
                    .px(px(10.))
                    .py(px(4.))
                    .truncate()
                    .text_size(theme::scaled_px(11.5))
                    .bg(theme::toolbar_surface())
                    .text_color(if has_selected {
                        theme::foreground()
                    } else {
                        theme::muted_foreground()
                    })
                    .border_b_1()
                    .border_color(theme::border())
                    .child(meta),
            )
            .when(!error.is_empty(), |section| {
                section.child(
                    div()
                        .id("collaborator-error-banner")
                        .test_support()
                        .px(px(10.))
                        .py(px(5.))
                        .text_size(theme::scaled_px(11.5))
                        .bg(theme::danger_bg())
                        .border_b_1()
                        .border_color(theme::danger_border())
                        .text_color(theme::danger_fg())
                        .child(error),
                )
            })
            .child(
                div()
                    .id("collaborator-raw-editor")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .child(editor_elem),
            )
            .child(search_bar)
            .into_any_element()
    }
}
