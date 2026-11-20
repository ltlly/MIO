use std::sync::{Arc, OnceLock};

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::{FontWeight, Image, ImageFormat, img};

use super::table::HIGHLIGHT_NAMES;
use super::*;

struct OrganiserRenameCollectionState {
    target_id: Option<mio_core::Uuid>,
    input: Entity<InputState>,
    _sub: Subscription,
}

impl OrganiserRenameCollectionState {
    fn new(owner: WeakEntity<Workbench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("输入新集合名称…"));
        let sub = cx.subscribe_in(&input, window, move |this, input, event, window, cx| {
            if !matches!(event, InputEvent::PressEnter { .. }) {
                return;
            }
            if input.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            }) {
                return;
            }
            let Some(cid) = this.target_id else {
                return;
            };
            let next_name = input.read(cx).value().to_string();
            let ok = owner
                .update(cx, |view, cx| {
                    view.rename_organiser_collection(cid, next_name, window, cx)
                })
                .unwrap_or(false);
            if ok {
                this.target_id = None;
                input.update(cx, |state, cx| state.set_value("", window, cx));
                cx.notify();
            }
        });
        Self {
            target_id: None,
            input,
            _sub: sub,
        }
    }
}

const EMPTY_CLIPBOARD_SVG_LIGHT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="256" height="200" viewBox="0 0 128 100">
  <polygon points="18,52 72,18 118,46 64,80" fill="#e1e5eb"/>
  <polygon points="18,44 64,72 64,79 18,51" fill="#3b5c85"/>
  <polygon points="64,72 118,38 118,45 64,79" fill="#2a466b"/>
  <polygon points="72,10 118,38 64,72 18,44" fill="#5c7fa8" stroke="#4f729c" stroke-width="1.6" stroke-linejoin="round"/>
  <polygon points="72,16 108,38 64,66 28,44" fill="#dce7f4" stroke="#c4d6ea" stroke-width="1" stroke-linejoin="round"/>
  <g stroke="#8da6c4" stroke-linecap="round">
    <line x1="65" y1="25" x2="82" y2="35" stroke-width="2.6"/>
    <line x1="60" y1="28" x2="72" y2="35" stroke-width="2.0"/>
    <line x1="54" y1="32" x2="71" y2="42" stroke-width="2.6"/>
    <line x1="49" y1="35" x2="62" y2="43" stroke-width="2.0"/>
    <line x1="43" y1="39" x2="59" y2="49" stroke-width="2.6"/>
    <line x1="38" y1="42" x2="49" y2="49" stroke-width="2.0"/>
  </g>
  <g fill="none" stroke="#517199" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round">
    <polyline points="86,39 85,42 94,41"/>
    <polyline points="75,46 74,49 83,48"/>
    <polyline points="64,53 63,56 72,55"/>
  </g>
  <polygon points="77,20 97,32 97,35 77,23" fill="#466891"/>
  <polygon points="97,32 105,27 105,30 97,35" fill="#395980"/>
  <polygon points="85,15 105,27 97,32 77,20" fill="#7091b9" stroke="#5b7da5" stroke-width="0.9" stroke-linejoin="round"/>
  <polygon points="95,14 105,20 99,24 89,18" fill="#88a7cc" stroke="#6d8eb5" stroke-width="0.8" stroke-linejoin="round"/>
  <ellipse cx="97" cy="19" rx="2.6" ry="1.6" transform="rotate(31 97 19)" fill="#587aa3"/>
</svg>"##;

const EMPTY_CLIPBOARD_SVG_DARK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="256" height="200" viewBox="0 0 128 100">
  <polygon points="18,52 72,18 118,46 64,80" fill="#1d1f23"/>
  <polygon points="18,44 64,72 64,79 18,51" fill="#2e4a6e"/>
  <polygon points="64,72 118,38 118,45 64,79" fill="#203754"/>
  <polygon points="72,10 118,38 64,72 18,44" fill="#4d7099" stroke="#42638a" stroke-width="1.6" stroke-linejoin="round"/>
  <polygon points="72,16 108,38 64,66 28,44" fill="#35465c" stroke="#465c78" stroke-width="1" stroke-linejoin="round"/>
  <g stroke="#7894b5" stroke-linecap="round">
    <line x1="65" y1="25" x2="82" y2="35" stroke-width="2.6"/>
    <line x1="60" y1="28" x2="72" y2="35" stroke-width="2.0"/>
    <line x1="54" y1="32" x2="71" y2="42" stroke-width="2.6"/>
    <line x1="49" y1="35" x2="62" y2="43" stroke-width="2.0"/>
    <line x1="43" y1="39" x2="59" y2="49" stroke-width="2.6"/>
    <line x1="38" y1="42" x2="49" y2="49" stroke-width="2.0"/>
  </g>
  <g fill="none" stroke="#9db7d6" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round">
    <polyline points="86,39 85,42 94,41"/>
    <polyline points="75,46 74,49 83,48"/>
    <polyline points="64,53 63,56 72,55"/>
  </g>
  <polygon points="77,20 97,32 97,35 77,23" fill="#39577c"/>
  <polygon points="97,32 105,27 105,30 97,35" fill="#2c4669"/>
  <polygon points="85,15 105,27 97,32 77,20" fill="#6384ac" stroke="#4e6f96" stroke-width="0.9" stroke-linejoin="round"/>
  <polygon points="95,14 105,20 99,24 89,18" fill="#7c9cc2" stroke="#6182a8" stroke-width="0.8" stroke-linejoin="round"/>
  <ellipse cx="97" cy="19" rx="2.6" ry="1.6" transform="rotate(31 97 19)" fill="#45658a"/>
</svg>"##;

fn empty_clipboard_image(dark: bool) -> Arc<Image> {
    if dark {
        static DARK: OnceLock<Arc<Image>> = OnceLock::new();
        DARK.get_or_init(|| {
            Arc::new(Image::from_bytes(
                ImageFormat::Svg,
                EMPTY_CLIPBOARD_SVG_DARK.to_vec(),
            ))
        })
        .clone()
    } else {
        static LIGHT: OnceLock<Arc<Image>> = OnceLock::new();
        LIGHT
            .get_or_init(|| {
                Arc::new(Image::from_bytes(
                    ImageFormat::Svg,
                    EMPTY_CLIPBOARD_SVG_LIGHT.to_vec(),
                ))
            })
            .clone()
    }
}

fn status_filter_icon(status: OrganiserStatus) -> IconName {
    match status {
        OrganiserStatus::Unknown => IconName::List,
        OrganiserStatus::New => IconName::CirclePlus,
        OrganiserStatus::InProgress => IconName::Activity,
        OrganiserStatus::Postponed => IconName::Pause,
        OrganiserStatus::Done => IconName::CircleCheck,
        OrganiserStatus::Ignored => IconName::Ban,
    }
}

fn status_filter_id(status: OrganiserStatus) -> &'static str {
    match status {
        OrganiserStatus::Unknown => "organiser-filter-unknown",
        OrganiserStatus::New => "organiser-filter-new",
        OrganiserStatus::InProgress => "organiser-filter-in-progress",
        OrganiserStatus::Postponed => "organiser-filter-postponed",
        OrganiserStatus::Done => "organiser-filter-done",
        OrganiserStatus::Ignored => "organiser-filter-ignored",
    }
}

fn neutral_mio_colors(dark: bool) -> (gpui_kit::Hsla, gpui_kit::Hsla, gpui_kit::Hsla, gpui_kit::Hsla) {
    if dark {
        (
            rgb(0x35383d).into(),
            rgb(0x40444a).into(),
            rgb(0x4b5058).into(),
            rgb(0x4e535b).into(),
        )
    } else {
        (
            rgb(0xeaeaea).into(),
            rgb(0xe0e0e0).into(),
            rgb(0xd6d6d6).into(),
            rgb(0xdcdcdc).into(),
        )
    }
}

impl Workbench {
    pub(in crate::workbench) fn render_organiser(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let heading_fg: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x141414).into()
        };
        let editable = self.organiser_editable();
        let show_empty_state = self.organiser_shows_empty_state();
        let total_count = self.workspace.organiser_items.len();

        let active_custom_name = self.organiser.active_collection.and_then(|cid| {
            self.organiser
                .custom_collections
                .iter()
                .find(|c| c.id == cid)
                .map(|c| c.name.clone())
        });
        let (header_icon, header_title) = if let Some(ref name) = active_custom_name {
            (IconName::Folder, name.clone())
        } else if let Some(status) = self.organiser.status_filter {
            (
                status_filter_icon(status),
                format!("收件箱 · {}", status.label()),
            )
        } else {
            (IconName::Inbox, "收件箱".to_owned())
        };

        let header = div()
            .id("organiser-header")
            .test_support()
            .w_full()
            .px(theme::scaled_px(18.))
            .pt(theme::scaled_px(20.))
            .pb(theme::scaled_px(14.))
            .flex()
            .items_center()
            .gap(theme::scaled_px(10.))
            .bg(theme::surface())
            .child(
                Icon::new(header_icon)
                    .size(theme::scaled_px(20.))
                    .text_color(heading_fg),
            )
            .child(
                div()
                    .text_size(theme::scaled_px(15.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(heading_fg)
                    .child(header_title),
            )
            .child(div().flex_1())
            .when(!show_empty_state, |row| {
                row.child(
                    div()
                        .id("organiser-count-badge")
                        .test_support()
                        .role(gpui_kit::accesskit::Role::Status)
                        .aria_label(format!("共 {total_count} 条收藏"))
                        .px(px(6.))
                        .text_size(theme::scaled_px(12.))
                        .text_color(theme::muted_foreground())
                        .child(format!("共 {total_count} 条")),
                )
                .child(
                    button("organiser-export-tsv", "导出 TSV")
                        .ghost()
                        .px(theme::scaled_px(8.))
                        .icon(IconName::Download)
                        .tooltip("将当前筛选后的条目列表以 TSV 制表符分隔格式复制到剪贴板")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.export_organiser_tsv(window, cx);
                        })),
                )
                .child(
                    button("organiser-prune-completed", "清理已结项")
                        .ghost()
                        .px(theme::scaled_px(8.))
                        .icon(IconName::Trash)
                        .tooltip("批量移除所有标记为「已完成」或「已忽略」的条目")
                        .disabled(!editable)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.clear_completed_organiser_items(window, cx);
                        })),
                )
            });

        let sub_tab_bar = div()
            .id("organiser-subtab-bar")
            .w_full()
            .px(theme::scaled_px(18.))
            .flex()
            .items_center()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .id("organiser-subtab-contents")
                    .test_support()
                    .role(gpui_kit::accesskit::Role::Tab)
                    .aria_selected(true)
                    .relative()
                    .h(theme::scaled_px(44.))
                    .pt(theme::scaled_px(6.))
                    .px(theme::scaled_px(14.))
                    .flex()
                    .items_center()
                    .text_size(theme::scaled_px(13.))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(heading_fg)
                    .child("内容")
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(3.))
                            .bg(theme::accent()),
                    ),
            );

        let right_body = if show_empty_state {
            self.render_organiser_empty_state(active_custom_name.as_deref(), window, cx)
        } else {
            self.render_organiser_populated_body(editable, cx)
        };

        div()
            .id("organiser-root")
            .test_support()
            .size_full()
            .flex()
            .flex_row()
            .bg(theme::surface())
            .child(self.render_organiser_sidebar(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(theme::surface())
                    .child(header)
                    .child(sub_tab_bar)
                    .child(right_body),
            )
            .into_any_element()
    }

    fn render_organiser_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let heading_fg: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x141414).into()
        };
        let (btn_bg, btn_hover, btn_active, btn_border) = neutral_mio_colors(dark);
        let field_bg: gpui_kit::Hsla = if dark {
            theme::background()
        } else {
            theme::surface()
        };
        let search_icon_fg: gpui_kit::Hsla = if dark {
            theme::muted_foreground()
        } else {
            rgb(0x2b2b2b).into()
        };
        let selected_bg: gpui_kit::Hsla = rgb(if dark { 0x2e436e } else { 0xd4e2f4 }).into();
        let hover_bg: gpui_kit::Hsla = rgb(if dark { 0x32353a } else { 0xf2f3f5 }).into();
        let editable = self.organiser_editable();
        let sidebar_owner = cx.weak_entity();
        let rename_state = window.use_keyed_state(
            "organiser-rename-collection-state",
            cx,
            |window, cx| OrganiserRenameCollectionState::new(sidebar_owner.clone(), window, cx),
        );
        let (renaming_id, rename_input) = {
            let st = rename_state.read(cx);
            (st.target_id, st.input.clone())
        };

        let query = self.organiser.collection_search_term.to_lowercase();
        let total_count = self.workspace.organiser_items.len();
        let inbox_selected =
            self.organiser.active_collection.is_none() && self.organiser.status_filter.is_none();

        let show_inbox = query.is_empty()
            || "收件箱".contains(&query)
            || "inbox".contains(&query)
            || "全部".contains(&query)
            || "all".contains(&query);

        let bg_owner = cx.weak_entity();
        let bg_rename = rename_state.clone();
        let mut list = div()
            .id("organiser-sidebar-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(theme::scaled_px(8.))
            .pb(theme::scaled_px(10.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .context_menu(move |menu, _, cx| {
                let (can_edit, active_col) = bg_owner
                    .upgrade()
                    .map(|entity| {
                        let view = entity.read(cx);
                        let active = view.organiser.active_collection.and_then(|cid| {
                            view.organiser
                                .custom_collections
                                .iter()
                                .find(|c| c.id == cid)
                                .map(|c| (c.id, c.name.clone()))
                        });
                        (view.organiser_editable(), active)
                    })
                    .unwrap_or((false, None));
                let o_new = bg_owner.clone();
                let o_del = bg_owner.clone();
                let o_exp = bg_owner.clone();
                let r_st = bg_rename.clone();
                let active_id_for_rename = active_col.as_ref().map(|(id, _)| *id);
                let active_name_for_rename = active_col.as_ref().map(|(_, n)| n.clone());
                let active_id_for_del = active_col.as_ref().map(|(id, _)| *id);
                let active_id_for_exp = active_col.as_ref().map(|(id, _)| *id);
                menu.compact()
                    .item(
                        PopupMenuItem::new("新建集合")
                            .icon(IconName::Plus)
                            .disabled(!can_edit)
                            .on_click(move |_, window, cx| {
                                let _ = o_new.update(cx, |this, cx| {
                                    this.toggle_create_organiser_collection(window, cx);
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("重命名集合")
                            .disabled(!can_edit || active_id_for_rename.is_none())
                            .on_click(move |_, window, cx| {
                                if let (Some(cid), Some(name)) =
                                    (active_id_for_rename, active_name_for_rename.clone())
                                {
                                    r_st.update(cx, |st, cx| {
                                        st.target_id = Some(cid);
                                        st.input.update(cx, |input, cx| {
                                            input.set_value(name, window, cx);
                                            input.focus(window, cx);
                                            input.select_all(window, cx);
                                        });
                                        cx.notify();
                                    });
                                }
                            }),
                    )
                    .item(
                        PopupMenuItem::new("删除集合")
                            .icon(IconName::Trash)
                            .disabled(!can_edit || active_id_for_del.is_none())
                            .on_click(move |_, window, cx| {
                                if let Some(cid) = active_id_for_del {
                                    let _ = o_del.update(cx, |this, cx| {
                                        this.delete_organiser_collection(cid, window, cx);
                                    });
                                }
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("导出集合…")
                            .icon(IconName::Download)
                            .disabled(!can_edit)
                            .on_click(move |_, window, cx| {
                                let _ = o_exp.update(cx, |this, cx| {
                                    this.export_organiser_collection(active_id_for_exp, window, cx);
                                });
                            }),
                    )
            });

        if show_inbox {
            list = list.child(self.render_sidebar_nav_item(
                "organiser-filter-all",
                "收件箱",
                "收件箱（全部）",
                "收件箱 — 显示全部工作流状态的收藏条目",
                IconName::Inbox,
                inbox_selected,
                total_count,
                selected_bg,
                hover_bg,
                heading_fg,
                cx.listener(|this, _, window, cx| {
                    this.set_organiser_status_filter(None, window, cx);
                }),
                window,
                cx,
            ));
        }

        for collection in &self.organiser.custom_collections {
            if !query.is_empty() && !collection.name.to_lowercase().contains(&query) {
                continue;
            }
            let cid = collection.id;
            let cname = collection.name.clone();
            let count = collection.item_ids.len();
            let is_selected = self.organiser.active_collection == Some(cid);
            let row_id = SharedString::from(format!("organiser-collection-{cid}"));
            let row_ctx_id = SharedString::from(format!("organiser-collection-row-{cid}"));
            let del_id = SharedString::from(format!("organiser-collection-delete-{cid}"));
            let tip_name = cname.clone();

            if renaming_id == Some(cid) {
                let r_confirm = rename_state.clone();
                let r_cancel = rename_state.clone();
                let o_confirm = cx.weak_entity();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("organiser-rename-row-{cid}")))
                        .test_support()
                        .w_full()
                        .py(px(2.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&rename_input)
                                    .id("organiser-rename-collection-input")
                                    .aria_label("重命名集合")
                                    .small()
                                    .h(theme::scaled_px(26.))
                                    .bg(field_bg),
                            ),
                        )
                        .child(
                            Button::new("organiser-rename-collection-confirm")
                                .ghost()
                                .small()
                                .h(theme::control_height())
                                .rounded(px(3.))
                                .px(px(5.))
                                .icon(IconName::Check)
                                .accessibility_label("确认重命名集合")
                                .tooltip("确认重命名（Enter）")
                                .on_click(move |_, window, cx| {
                                    let next_name = r_confirm
                                        .read(cx)
                                        .input
                                        .read(cx)
                                        .value()
                                        .to_string();
                                    let ok = o_confirm
                                        .update(cx, |this, cx| {
                                            this.rename_organiser_collection(
                                                cid, next_name, window, cx,
                                            )
                                        })
                                        .unwrap_or(false);
                                    if ok {
                                        r_confirm.update(cx, |st, cx| {
                                            st.target_id = None;
                                            st.input.update(cx, |inp, cx| {
                                                inp.set_value("", window, cx);
                                            });
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                        .child(
                            Button::new("organiser-rename-collection-cancel")
                                .ghost()
                                .small()
                                .h(theme::control_height())
                                .rounded(px(3.))
                                .px(px(5.))
                                .icon(IconName::Close)
                                .accessibility_label("取消重命名集合")
                                .tooltip("取消重命名")
                                .on_click(move |_, window, cx| {
                                    r_cancel.update(cx, |st, cx| {
                                        st.target_id = None;
                                        st.input.update(cx, |inp, cx| {
                                            inp.set_value("", window, cx);
                                        });
                                        cx.notify();
                                    });
                                }),
                        ),
                );
                continue;
            }

            let col_owner = cx.weak_entity();
            let col_rename_st = rename_state.clone();
            let col_name_for_menu = cname.clone();
            list = list.child(
                div()
                    .id(row_ctx_id)
                    .test_support()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .context_menu(move |menu, _, _| {
                        let o_new = col_owner.clone();
                        let o_del = col_owner.clone();
                        let o_exp = col_owner.clone();
                        let r_st = col_rename_st.clone();
                        let rename_seed = col_name_for_menu.clone();
                        menu.compact()
                            .item(
                                PopupMenuItem::new("新建集合")
                                    .icon(IconName::Plus)
                                    .disabled(!editable)
                                    .on_click(move |_, window, cx| {
                                        let _ = o_new.update(cx, |this, cx| {
                                            this.toggle_create_organiser_collection(window, cx);
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("重命名集合")
                                    .disabled(!editable)
                                    .on_click(move |_, window, cx| {
                                        let seed = rename_seed.clone();
                                        r_st.update(cx, |st, cx| {
                                            st.target_id = Some(cid);
                                            st.input.update(cx, |input, cx| {
                                                input.set_value(seed, window, cx);
                                                input.focus(window, cx);
                                                input.select_all(window, cx);
                                            });
                                            cx.notify();
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("删除集合")
                                    .icon(IconName::Trash)
                                    .disabled(!editable)
                                    .on_click(move |_, window, cx| {
                                        let _ = o_del.update(cx, |this, cx| {
                                            this.delete_organiser_collection(cid, window, cx);
                                        });
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("导出集合…")
                                    .icon(IconName::Download)
                                    .disabled(!editable)
                                    .on_click(move |_, window, cx| {
                                        let _ = o_exp.update(cx, |this, cx| {
                                            this.export_organiser_collection(Some(cid), window, cx);
                                        });
                                    }),
                            )
                    })
                    .child(
                        div().flex_1().min_w_0().child(self.render_sidebar_nav_item(
                            row_id,
                            cname.clone(),
                            cname,
                            format!("查看集合「{tip_name}」中的条目"),
                            IconName::Folder,
                            is_selected,
                            count,
                            selected_bg,
                            hover_bg,
                            heading_fg,
                            cx.listener(move |this, _, window, cx| {
                                this.select_organiser_collection(Some(cid), window, cx);
                            }),
                            window,
                            cx,
                        )),
                    )
                    .child(
                        flat_button(del_id, false, window, cx)
                            .accessibility_label("删除集合")
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("删除此集合")
                                    .build(window, cx)
                            })
                            .w(theme::scaled_px(22.))
                            .h(theme::scaled_px(26.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme::muted_foreground())
                            .hover(move |s| s.bg(hover_bg).text_color(heading_fg))
                            .child(Icon::new(IconName::Close).size(theme::scaled_px(12.)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.delete_organiser_collection(cid, window, cx);
                            })),
                    ),
            );
        }

        div()
            .id("organiser-sidebar")
            .test_support()
            .w(theme::scaled_px(250.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .border_r_1()
            .border_color(theme::border())
            .child(
                div()
                    .w_full()
                    .px(theme::scaled_px(12.))
                    .pt(theme::scaled_px(10.))
                    .pb(theme::scaled_px(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(theme::scaled_px(15.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(heading_fg)
                            .child("集合"),
                    )
                    .child(
                        flat_button("organiser-new-collection", false, window, cx)
                            .accessibility_label("新建集合")
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    "新建一个自定义 Organizer 集合",
                                )
                                .build(window, cx)
                            })
                            .h(theme::scaled_px(27.))
                            .px(theme::scaled_px(12.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .bg(btn_bg)
                            .border_1()
                            .border_color(btn_border)
                            .hover(move |s| s.bg(btn_hover))
                            .active(move |s| s.bg(btn_active))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(5.))
                            .text_size(theme::scaled_px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(heading_fg)
                            .child(Icon::new(IconName::Plus).size(theme::scaled_px(13.)))
                            .child("新建")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_create_organiser_collection(window, cx);
                            })),
                    ),
            )
            .when(self.organiser.creating_collection, |col| {
                col.child(
                    div()
                        .px(theme::scaled_px(10.))
                        .pb(theme::scaled_px(8.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&self.organiser.new_collection_input)
                                    .id("organiser-new-collection-input")
                                    .aria_label("新集合名称")
                                    .small()
                                    .h(theme::scaled_px(26.))
                                    .bg(field_bg),
                            ),
                        )
                        .child(
                            Button::new("organiser-new-collection-confirm")
                                .ghost()
                                .small()
                                .h(theme::control_height())
                                .rounded(px(3.))
                                .px(px(5.))
                                .icon(IconName::Check)
                                .accessibility_label("确认新建集合")
                                .tooltip("确认创建集合（Enter）")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_create_organiser_collection(window, cx);
                                })),
                        )
                        .child(
                            Button::new("organiser-new-collection-cancel")
                                .ghost()
                                .small()
                                .h(theme::control_height())
                                .rounded(px(3.))
                                .px(px(5.))
                                .icon(IconName::Close)
                                .accessibility_label("取消新建集合")
                                .tooltip("取消创建集合")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.cancel_create_organiser_collection(window, cx);
                                })),
                        ),
                )
            })
            .child(
                div()
                    .px(theme::scaled_px(10.))
                    .pb(theme::scaled_px(10.))
                    .child(
                        Input::new(&self.organiser.collection_search)
                            .id("organiser-collection-search")
                            .aria_label("搜索集合")
                            .small()
                            .h(theme::scaled_px(28.))
                            .bg(field_bg)
                            .suffix(
                                Icon::new(IconName::Search)
                                    .size(theme::scaled_px(14.))
                                    .text_color(search_icon_fg),
                            ),
                    ),
            )
            .child(list)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_sidebar_nav_item(
        &self,
        id: impl Into<gpui_kit::ElementId>,
        label: impl Into<SharedString>,
        a11y_label: impl Into<SharedString>,
        tip: impl Into<SharedString>,
        icon: IconName,
        selected: bool,
        count: usize,
        selected_bg: gpui_kit::Hsla,
        hover_bg: gpui_kit::Hsla,
        heading_fg: gpui_kit::Hsla,
        on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tip: SharedString = tip.into();
        let row_fg = if selected {
            heading_fg
        } else {
            theme::foreground()
        };
        flat_button(id, false, window, cx)
            .accessibility_label(a11y_label.into())
            .aria_selected(selected)
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .w_full()
            .h(theme::scaled_px(30.))
            .px(theme::scaled_px(10.))
            .rounded(px(5.))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(theme::scaled_px(8.))
            .bg(if selected {
                selected_bg
            } else {
                gpui_kit::transparent_black()
            })
            .hover(move |s| s.bg(if selected { selected_bg } else { hover_bg }))
            .text_size(theme::scaled_px(13.))
            .font_weight(FontWeight::NORMAL)
            .text_color(row_fg)
            .child(Icon::new(icon).size(theme::scaled_px(15.)).text_color(heading_fg))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_left()
                    .child(label.into()),
            )
            .when(count > 0, |row| {
                row.child(
                    div()
                        .text_size(theme::scaled_px(11.5))
                        .text_color(theme::muted_foreground())
                        .child(count.to_string()),
                )
            })
            .on_click(on_click)
            .into_any_element()
    }

    fn render_organiser_empty_state(
        &self,
        custom_collection_name: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let heading_fg: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x141414).into()
        };
        let desc_fg: gpui_kit::Hsla = if dark {
            theme::muted_foreground()
        } else {
            rgb(0x3a3a3a).into()
        };
        let (btn_bg, btn_hover, btn_active, btn_border) = neutral_mio_colors(dark);

        let (title, line1, line2) = if let Some(name) = custom_collection_name {
            (
                format!("集合「{name}」为空"),
                "在收件箱条目上右键选择「添加到集合」，",
                "即可将报文整理到此集合中。",
            )
        } else {
            (
                "Organizer 收件箱为空".to_owned(),
                "从工作台任意位置将消息发送到 Organizer",
                "即可开始使用。",
            )
        };

        div()
            .id("organiser-empty")
            .test_support()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(24.))
            .pb(px(36.))
            .bg(theme::surface())
            .child(
                div()
                    .max_w(px(460.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        img(empty_clipboard_image(dark))
                            .w(theme::scaled_px(124.))
                            .h(theme::scaled_px(97.))
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .mt(theme::scaled_px(18.))
                            .text_size(theme::scaled_px(18.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(heading_fg)
                            .child(title),
                    )
                    .child(
                        div()
                            .mt(theme::scaled_px(12.))
                            .flex()
                            .flex_col()
                            .items_center()
                            .text_size(theme::scaled_px(13.))
                            .line_height(theme::scaled_px(19.))
                            .text_color(desc_fg)
                            .child(line1)
                            .child(line2),
                    )
                    .child(
                        flat_button("organiser-learn-more", false, window, cx)
                            .accessibility_label("了解更多")
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    "查看 Organizer 使用说明",
                                )
                                .build(window, cx)
                            })
                            .mt(theme::scaled_px(20.))
                            .min_w(theme::scaled_px(114.))
                            .h(theme::scaled_px(28.))
                            .px(theme::scaled_px(18.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .bg(btn_bg)
                            .border_1()
                            .border_color(btn_border)
                            .hover(move |s| s.bg(btn_hover))
                            .active(move |s| s.bg(btn_active))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(theme::scaled_px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(heading_fg)
                            .child("了解更多")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.organiser.learn_more_open = !this.organiser.learn_more_open;
                                cx.notify();
                            })),
                    )
                    .when(self.organiser.learn_more_open, |col| {
                        col.child(
                            div()
                                .id("organiser-learn-more-card")
                                .test_support()
                                .mt(theme::scaled_px(16.))
                                .w_full()
                                .p(theme::scaled_px(14.))
                                .rounded(px(6.))
                                .bg(theme::toolbar_surface())
                                .border_1()
                                .border_color(theme::border())
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .text_size(theme::scaled_px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(heading_fg)
                                        .child("如何使用 Organizer 整理测试报文"),
                                )
                                .child(
                                    div()
                                        .text_size(theme::scaled_px(12.))
                                        .line_height(theme::scaled_px(18.))
                                        .text_color(theme::muted_foreground())
                                        .child("• 在 Proxy、Target、Repeater、Intruder 或 Logger 中右键消息并选择「发送到 Organizer」，即可归档原始请求与响应。"),
                                )
                                .child(
                                    div()
                                        .text_size(theme::scaled_px(12.))
                                        .line_height(theme::scaled_px(18.))
                                        .text_color(theme::muted_foreground())
                                        .child("• 在左侧新建自定义集合，并在右侧内容工具栏按状态筛选、维护研判状态、高亮标记与多行分析备注。"),
                                )
                                .child(
                                    div()
                                        .mt(px(4.))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(
                                            button("organiser-goto-proxy", "前往 Proxy 历史")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.select_tool(Tool::Proxy, window, cx);
                                                    this.proxy_page = ProxyPage::History;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            button("organiser-learn-more-close", "收起")
                                                .ghost()
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.organiser.learn_more_open = false;
                                                    cx.notify();
                                                })),
                                        ),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_organiser_populated_body(
        &self,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let field_bg: gpui_kit::Hsla = if dark {
            theme::background()
        } else {
            theme::surface()
        };
        let selected = self.organiser.selected_id.and_then(|id| {
            self.workspace
                .organiser_items
                .iter()
                .find(|item| item.id == id)
        });
        let has_any_selection = selected.is_some() || !self.organiser.selected_ids.is_empty();
        let has_response = selected.is_some_and(|item| item.response_raw.is_some());

        let divider = || {
            div()
                .w(px(1.))
                .h(px(16.))
                .mx(px(3.))
                .flex_shrink_0()
                .bg(theme::border())
        };

        let active_status = if self.organiser.active_collection.is_none() {
            self.organiser.status_filter
        } else {
            None
        };
        let status_filter_buttons = OrganiserStatus::ALL.into_iter().map(|status| {
            let id = status_filter_id(status);
            let label = status.label();
            let is_active = active_status == Some(status);
            button(id, label)
                .ghost()
                .px(theme::scaled_px(6.))
                .tooltip(format!("仅显示工作流状态为「{label}」的收藏条目"))
                .selected(is_active)
                .on_click(cx.listener(move |this, _, window, cx| {
                    let next = if this.organiser.active_collection.is_none()
                        && this.organiser.status_filter == Some(status)
                    {
                        None
                    } else {
                        Some(status)
                    };
                    this.set_organiser_status_filter(next, window, cx);
                }))
        });

        let filter_action_bar = div()
            .id("organiser-toolbar")
            .test_support()
            .w_full()
            .min_w_0()
            .px(theme::scaled_px(12.))
            .py(px(5.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(3.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div().w(theme::scaled_px(200.)).min_w(px(140.)).child(
                    Input::new(&self.organiser.search)
                        .id("organiser-search")
                        .aria_label("搜索 Organiser 条目")
                        .small()
                        .h(theme::scaled_px(25.))
                        .bg(field_bg)
                        .suffix(
                            Icon::new(IconName::Search)
                                .size(theme::scaled_px(13.))
                                .text_color(theme::muted_foreground()),
                        ),
                ),
            )
            .child(divider())
            .children(status_filter_buttons)
            .child(divider())
            .child(self.render_organiser_status_menu(editable, has_any_selection, cx))
            .child(self.render_organiser_highlight_menu(editable, has_any_selection, cx))
            .child(
                button("organiser-select-all", "全选可见")
                    .ghost()
                    .px(theme::scaled_px(7.))
                    .icon(IconName::Check)
                    .tooltip("选中当前筛选结果列表中的全部可见条目")
                    .disabled(!editable || self.workspace.organiser_items.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_all_visible_organiser_items(window, cx);
                    })),
            )
            .child(divider())
            .child(
                button("organiser-to-repeater", "Repeater")
                    .ghost()
                    .px(theme::scaled_px(7.))
                    .icon(IconName::Repeat)
                    .tooltip("将当前选中条目的请求报文发送到 Repeater 重放")
                    .disabled(!editable || selected.is_none())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.send_organiser_to_repeater(window, cx);
                    })),
            )
            .child(
                button("organiser-to-intruder", "Intruder")
                    .ghost()
                    .px(theme::scaled_px(7.))
                    .icon(IconName::Crosshair)
                    .tooltip("将当前选中条目的请求报文发送到 Intruder 配置攻击")
                    .disabled(!editable || selected.is_none())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.send_organiser_to_intruder(window, cx);
                    })),
            )
            .child(
                button("organiser-to-comparer", "Comparer")
                    .ghost()
                    .px(theme::scaled_px(7.))
                    .icon(IconName::GitCompare)
                    .tooltip("将当前选中条目的原始请求字节送入 Comparer 对比")
                    .disabled(!editable || selected.is_none())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.send_organiser_to_comparer(false, window, cx);
                    })),
            )
            .when(has_response, |row| {
                row.child(
                    button("organiser-to-comparer-resp", "响应→Comparer")
                        .ghost()
                        .px(theme::scaled_px(7.))
                        .icon(IconName::GitCompare)
                        .tooltip("将当前选中条目的原始响应字节送入 Comparer 对比")
                        .disabled(!editable)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.send_organiser_to_comparer(true, window, cx);
                        })),
                )
            })
            .child(divider())
            .child(
                button("organiser-copy-url", "复制 URL")
                    .ghost()
                    .px(theme::scaled_px(7.))
                    .icon(IconName::Copy)
                    .tooltip("复制当前选中条目的完整请求 URL 到剪贴板")
                    .disabled(!editable || selected.is_none())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.copy_organiser_url(window, cx);
                    })),
            )
            .child(
                button("organiser-copy-curl", "curl")
                    .ghost()
                    .px(theme::scaled_px(7.))
                    .icon(IconName::SquareTerminal)
                    .tooltip("将当前选中条目的请求生成为等效 curl 命令并复制")
                    .disabled(!editable || selected.is_none())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.copy_organiser_curl(window, cx);
                    })),
            )
            .child(
                button("organiser-delete-item", "删除")
                    .ghost()
                    .px(theme::scaled_px(7.))
                    .icon(IconName::Trash)
                    .tooltip("从 Organiser 中移除当前选中的条目")
                    .disabled(!editable || !has_any_selection)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.delete_selected_organiser_batch(window, cx);
                    })),
            );

        let table_pane = div()
            .id("organiser-table-pane")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .child(
                DataTable::new(&self.organiser.table)
                    .with_size(theme::row_height())
                    .header_row_height(theme::table_header_height())
                    .stripe(true)
                    .bordered(false)
                    .scroll_bounce(false)
                    .accessibility_label("Organiser 收藏条目"),
            );

        let messages = self.render_context_message_pair(
            "organiser",
            &self.organiser.request,
            &self.organiser.response,
            true,
            cx,
        );
        let show_inspector = self.inspector.open;
        let show_notes = !show_inspector && self.organiser.notes_open;
        let notes_card = show_notes.then(|| {
            // Avoid cloning multi-MB request/response raw byte buffers for the metadata-only notes card.
            let selected_meta = selected.map(|item| OrganiserItem {
                id: item.id,
                number: item.number,
                added_unix_ms: item.added_unix_ms,
                source_tool: String::new(),
                target: String::new(),
                method: String::new(),
                url: String::new(),
                status_code: item.status_code,
                response_len: item.response_len,
                request_raw: Vec::new(),
                response_raw: None,
                http_version: item.http_version,
                workflow_status: item.workflow_status,
                notes: item.notes.clone(),
                highlight: item.highlight,
            });
            self.render_organiser_notes_card(selected_meta, editable, cx)
        });

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(filter_action_bar)
            .child(
                div().flex_1().min_h_0().w_full().child(
                    v_resizable("organiser-split")
                        .child(
                            resizable_panel()
                                .size(px(260.))
                                .size_range(px(120.)..px(900.))
                                .child(table_pane),
                        )
                        .child(
                            resizable_panel().size_range(px(120.)..px(10000.)).child(
                                div()
                                    .size_full()
                                    .flex()
                                    .child(div().flex_1().min_w_0().child(messages))
                                    .when(show_inspector, |row| {
                                        row.child(self.render_inspector_sidebar(cx))
                                    })
                                    .when_some(notes_card, |row, card| row.child(card))
                                    .child(
                                        sidebar_rail()
                                            .child(
                                                sidebar_rail_tab(
                                                    "inspector-toggle",
                                                    "检\n视",
                                                    "Inspector",
                                                    IconName::SlidersHorizontal,
                                                    show_inspector,
                                                    false,
                                                    if show_inspector {
                                                        "收起 Inspector 检视面板"
                                                    } else {
                                                        "展开 Inspector 检视面板"
                                                    },
                                                )
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    if this.inspector.open {
                                                        if !this
                                                            .dismiss_inspector_for_current_selection(
                                                                window, cx,
                                                            )
                                                        {
                                                            return;
                                                        }
                                                    } else {
                                                        if !this.open_inspector(window, cx) {
                                                            return;
                                                        }
                                                        this.organiser.notes_open = false;
                                                    }
                                                    cx.notify();
                                                })),
                                            )
                                            .child(
                                                sidebar_rail_tab(
                                                    "organiser-notes-toggle",
                                                    "备\n注",
                                                    "备注与状态",
                                                    IconName::FileText,
                                                    show_notes,
                                                    false,
                                                    if show_notes {
                                                        "收起条目备注与状态侧栏"
                                                    } else {
                                                        "展开条目备注与状态侧栏"
                                                    },
                                                )
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    if let Err(error) = this
                                                        .validate_organiser_composition(window, cx)
                                                    {
                                                        this.fail(error, cx);
                                                        return;
                                                    }
                                                    let notes_open = !this.inspector.open
                                                        && this.organiser.notes_open;
                                                    if !notes_open
                                                        && !this
                                                            .dismiss_inspector_for_current_selection(
                                                                window, cx,
                                                            )
                                                    {
                                                        return;
                                                    }
                                                    if notes_open
                                                        && this
                                                            .organiser
                                                            .notes
                                                            .focus_handle(cx)
                                                            .is_focused(window)
                                                    {
                                                        this.focus.focus(window, cx);
                                                    }
                                                    this.organiser.notes_open = !notes_open;
                                                    cx.notify();
                                                })),
                                            ),
                                    ),
                            ),
                        ),
                ),
            )
            .into_any_element()
    }

    fn render_organiser_status_menu(
        &self,
        editable: bool,
        has_selection: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = cx.weak_entity();
        let disabled = !editable || !has_selection;
        button("organiser-status-menu", "状态")
            .ghost()
            .px(theme::scaled_px(7.))
            .icon(IconName::Tag)
            .dropdown_caret(true)
            .tooltip("批量设置选中条目的研判工作流状态")
            .disabled(disabled)
            .dropdown_menu(move |menu, _, cx| {
                let Some(entity) = owner.upgrade() else {
                    return menu;
                };
                let view = entity.read(cx);
                let enabled = view.organiser_editable()
                    && (view.organiser.selected_id.is_some()
                        || !view.organiser.selected_ids.is_empty());
                let multi = view.organiser.selected_ids.len() > 1;
                let current = view
                    .organiser
                    .selected_id
                    .and_then(|id| {
                        view.workspace
                            .organiser_items
                            .iter()
                            .find(|item| item.id == id)
                    })
                    .map(|item| item.workflow_status)
                    .unwrap_or(OrganiserStatus::Unknown);
                OrganiserStatus::ALL.into_iter().fold(menu, |menu, status| {
                    let owner = owner.clone();
                    menu.item(
                        PopupMenuItem::new(status.label())
                            .disabled(!enabled)
                            .checked(!multi && current == status)
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.set_selected_organiser_status(status, window, cx);
                                });
                            }),
                    )
                })
            })
            .into_any_element()
    }

    fn render_organiser_highlight_menu(
        &self,
        editable: bool,
        has_selection: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = cx.weak_entity();
        let disabled = !editable || !has_selection;
        button("organiser-highlight-menu", "高亮")
            .ghost()
            .px(theme::scaled_px(7.))
            .icon(IconName::Palette)
            .dropdown_caret(true)
            .tooltip("批量设置选中条目的高亮标记颜色")
            .disabled(disabled)
            .dropdown_menu(move |menu, _, cx| {
                let Some(entity) = owner.upgrade() else {
                    return menu;
                };
                let view = entity.read(cx);
                let enabled = view.organiser_editable()
                    && (view.organiser.selected_id.is_some()
                        || !view.organiser.selected_ids.is_empty());
                let multi = view.organiser.selected_ids.len() > 1;
                let current = view
                    .organiser
                    .selected_id
                    .and_then(|id| {
                        view.workspace
                            .organiser_items
                            .iter()
                            .find(|item| item.id == id)
                    })
                    .and_then(|item| item.highlight)
                    .unwrap_or(0);
                (0u8..=9).fold(menu, |menu, color| {
                    let owner = owner.clone();
                    let target = (color > 0).then_some(color);
                    menu.item(
                        PopupMenuItem::new(HIGHLIGHT_NAMES[usize::from(color)])
                            .disabled(!enabled)
                            .checked(!multi && current == color)
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.set_selected_organiser_highlight(target, window, cx);
                                });
                            }),
                    )
                })
            })
            .into_any_element()
    }
}
