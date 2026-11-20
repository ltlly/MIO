use std::sync::{Arc, OnceLock};

use gpui_kit::{
    AnyElement, AnyWindowHandle, App, Bounds, Entity, Image, ImageFormat, IntoElement, Pixels,
    Render, SharedString, TitlebarOptions, Window, WindowBounds, WindowDecorations, WindowKind,
    WindowOptions, div, point, px,
};
#[cfg(not(test))]
use gpui_kit::{
    Context, ElementId, FontWeight, InteractiveElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, WindowControlArea,
    component::{Icon, IconName, Sizable as _, menu::AppMenuBar},
    img,
    prelude::FluentBuilder as _,
    rgb,
};

#[cfg(not(test))]
use crate::theme;

pub(crate) const CUSTOM_TITLE_BAR_HEIGHT: f32 = 32.0;

const LOGO_PNG_BYTES: &[u8] = include_bytes!("../assets/mio-logo-128.png");
const LOGO_RGBA64_BYTES: &[u8] = include_bytes!("../assets/mio-logo-64.rgba");

pub(crate) fn logo_image() -> Arc<Image> {
    static LOGO: OnceLock<Arc<Image>> = OnceLock::new();
    LOGO.get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Png, LOGO_PNG_BYTES.to_vec())))
        .clone()
}

pub(crate) fn window_icon() -> Arc<image::RgbaImage> {
    static ICON: OnceLock<Arc<image::RgbaImage>> = OnceLock::new();
    ICON.get_or_init(|| {
        Arc::new(
            image::RgbaImage::from_raw(64, 64, LOGO_RGBA64_BYTES.to_vec())
                .expect("valid 64x64 RGBA MIO icon"),
        )
    })
    .clone()
}

pub(crate) fn mio_window_options(
    bounds: Bounds<Pixels>,
    resizable: bool,
    minimizable: bool,
) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(point(px(11.0), px(9.0))),
        }),
        app_owns_titlebar_drag: true,
        is_resizable: resizable,
        is_minimizable: minimizable,
        window_decorations: Some(WindowDecorations::Client),
        icon: Some(window_icon()),
        app_id: Some("mio".into()),
        ..Default::default()
    }
}

fn prepare_popup_options(mut options: WindowOptions) -> WindowOptions {
    // Non-floating normal window that still focuses/raises on open above parent windows.
    options.kind = WindowKind::Normal;
    options.focus = true;
    options.show = true;
    options
}

pub(crate) fn activate_popup_window(handle: AnyWindowHandle, cx: &mut App) -> bool {
    handle
        .update(cx, |_, window, cx| {
            window.activate_window();
            cx.activate(true);
        })
        .is_ok()
}

pub(crate) fn open_popup_window<V: Render + 'static>(
    options: WindowOptions,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> anyhow::Result<(AnyWindowHandle, Entity<V>)> {
    let opened = gpui_kit::open_window(prepare_popup_options(options), cx, build)?;
    let handle = opened.0;
    let _ = handle.update(cx, |_, window, cx| {
        window.activate_window();
        cx.activate(true);
    });
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, cx| {
            window.activate_window();
            cx.activate(true);
        });
    });
    Ok(opened)
}

#[derive(Clone, Copy)]
pub(crate) struct TitleBarControls {
    pub minimize: bool,
    pub maximize: bool,
}

impl TitleBarControls {
    pub const FULL: Self = Self {
        minimize: true,
        maximize: true,
    };
    pub const DIALOG: Self = Self {
        minimize: false,
        maximize: false,
    };
}

#[cfg(not(test))]
struct DragState {
    dragging: bool,
}

#[cfg(not(test))]
impl Render for DragState {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub(crate) fn render_title_bar(
    id_prefix: &'static str,
    title: impl Into<SharedString>,
    controls: TitleBarControls,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    #[cfg(test)]
    {
        let _ = (
            id_prefix,
            title,
            controls.minimize,
            controls.maximize,
            on_close,
            window,
            cx,
        );
        div().into_any_element()
    }

    #[cfg(not(test))]
    {
        #[cfg(target_os = "linux")]
        if matches!(window.window_decorations(), gpui_kit::Decorations::Server) {
            return div().into_any_element();
        }
        let title = title.into();
        let dark = theme::is_dark();
        let bar_bg = rgb(if dark { 0x24262a } else { 0xffffff });
        let btn_hover = rgb(if dark { 0x383c42 } else { 0xe2e5ea });
        let btn_active = rgb(if dark { 0x464b52 } else { 0xd4d8df });
        let is_macos = cfg!(target_os = "macos");
        let is_windows = cfg!(target_os = "windows");
        let is_linux = cfg!(target_os = "linux");
        let can_maximize = controls.maximize;

        let left_pad = if is_macos && !window.is_fullscreen() {
            px(84.)
        } else {
            px(12.)
        };

        let drag_state = window.use_keyed_state(
            ElementId::Name(SharedString::from(format!("{id_prefix}-drag-state"))),
            cx,
            |_, _| DragState { dragging: false },
        );
        let s1 = drag_state.clone();
        let s2 = drag_state.clone();
        let s3 = drag_state.clone();
        let s4 = drag_state;

        let on_close = std::rc::Rc::new(on_close);

        let drag_region = div()
            .id(ElementId::Name(SharedString::from(format!(
                "{id_prefix}-drag"
            ))))
            .h_full()
            .flex_1()
            .min_w_0()
            .pl(left_pad)
            .pr(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .window_control_area(WindowControlArea::Drag)
            .when(is_macos, |el| {
                el.on_click(|ev, window, _| {
                    if ev.click_count() == 2 {
                        window.titlebar_double_click();
                    }
                })
            })
            .when(is_linux && can_maximize, |el| {
                el.on_click(|ev, window, _| {
                    if ev.click_count() == 2 {
                        window.zoom_window();
                    }
                })
            })
            .on_mouse_down_out(move |_, _, cx| {
                s1.update(cx, |st, _| st.dragging = false);
            })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                s2.update(cx, |st, _| st.dragging = true);
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                s3.update(cx, |st, _| st.dragging = false);
            })
            .on_mouse_move(move |_, window, cx| {
                if s4.read(cx).dragging {
                    s4.update(cx, |st, _| st.dragging = false);
                    window.start_window_move();
                }
            })
            .when(is_linux, |el| {
                el.on_mouse_down(MouseButton::Right, |ev, window, _| {
                    window.show_window_menu(ev.position);
                })
            })
            .when(!is_macos, |region| {
                region.child(
                    img(logo_image())
                        .w(theme::scaled_px(20.).max(px(16.)))
                        .h(theme::scaled_px(20.).max(px(16.)))
                        .flex_shrink_0(),
                )
            })
            .child(
                div()
                    .when(!is_macos && controls.maximize, |d| {
                        d.flex_shrink_0().max_w(px(260.))
                    })
                    .when(is_macos || !controls.maximize, |d| d.flex_1().min_w_0())
                    .truncate()
                    .font_family(theme::ui_font_family())
                    .text_size(theme::scaled_px(13.))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(rgb(if dark { 0xdfe1e5 } else { 0x5a5a5a }))
                    .child(title),
            )
            .when(!is_macos && controls.maximize, |el| {
                let app_menu_bar = window
                    .use_keyed_state(
                        ElementId::Name(SharedString::from(format!("{id_prefix}-app-menu-bar"))),
                        cx,
                        |_, cx| AppMenuBar::new(cx),
                    )
                    .read(cx)
                    .clone();
                el.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .pl(px(8.))
                        .flex()
                        .items_center()
                        .child(app_menu_bar),
                )
            });

        let mut bar = div()
            .id(ElementId::Name(SharedString::from(format!(
                "{id_prefix}-titlebar"
            ))))
            .h(theme::scaled_px(CUSTOM_TITLE_BAR_HEIGHT).max(px(28.)))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .bg(bar_bg)
            .border_b_1()
            .border_color(if dark { theme::border() } else { rgb(0xe2e2e2).into() })
            .child(drag_region);

        if !is_macos {
            let supported = window.window_controls();
            let show_min = controls.minimize && (is_windows || supported.minimize);
            let show_max = controls.maximize && (is_windows || supported.maximize);
            let is_max = window.is_maximized();
            let close_cb = on_close;

            let controls_box = div()
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .when(show_min, |row| {
                    row.child(
                        div()
                            .id(ElementId::Name(SharedString::from(format!(
                                "{id_prefix}-win-min"
                            ))))
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("最小化窗口")
                                    .build(window, cx)
                            })
                            .w(px(42.))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme::foreground())
                            .hover(move |s| s.bg(btn_hover))
                            .active(move |s| s.bg(btn_active))
                            .when(is_windows, |b| {
                                b.window_control_area(WindowControlArea::Min)
                            })
                            .when(is_linux, |b| {
                                b.on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                })
                                .on_click(|_, window, cx| {
                                    cx.stop_propagation();
                                    window.minimize_window();
                                })
                            })
                            .child(Icon::new(IconName::WindowMinimize).small()),
                    )
                })
                .when(show_max, |row| {
                    row.child(
                        div()
                            .id(ElementId::Name(SharedString::from(format!(
                                "{id_prefix}-win-max"
                            ))))
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("最大化或还原窗口")
                                    .build(window, cx)
                            })
                            .w(px(42.))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme::foreground())
                            .hover(move |s| s.bg(btn_hover))
                            .active(move |s| s.bg(btn_active))
                            .when(is_windows, |b| {
                                b.window_control_area(WindowControlArea::Max)
                            })
                            .when(is_linux, |b| {
                                b.on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                })
                                .on_click(|_, window, cx| {
                                    cx.stop_propagation();
                                    window.zoom_window();
                                })
                            })
                            .child(
                                Icon::new(if is_max {
                                    IconName::WindowRestore
                                } else {
                                    IconName::WindowMaximize
                                })
                                .small(),
                            ),
                    )
                })
                .child(
                    div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "{id_prefix}-win-close"
                        ))))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("关闭窗口").build(window, cx)
                        })
                        .w(px(44.))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(theme::foreground())
                        .hover(|s| s.bg(rgb(0xe81123)).text_color(rgb(0xffffff)))
                        .active(|s| s.bg(rgb(0xc50f1f)).text_color(rgb(0xffffff)))
                        .when(is_windows, |b| {
                            b.window_control_area(WindowControlArea::Close)
                        })
                        .when(is_linux, move |b| {
                            b.on_mouse_down(MouseButton::Left, |_, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                            })
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                close_cb(window, cx);
                            })
                        })
                        .child(Icon::new(IconName::WindowClose).small()),
                );
            bar = bar.child(controls_box);
        }

        bar.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::size;

    #[test]
    fn embedded_mio_bowl_logo_and_custom_window_options_are_valid() {
        let icon = window_icon();
        assert_eq!((icon.width(), icon.height()), (64, 64));
        let logo = logo_image();
        assert_eq!(logo.format(), ImageFormat::Png);
        assert!(!logo.bytes().is_empty());

        let opts = mio_window_options(
            Bounds {
                origin: point(px(10.), px(20.)),
                size: size(px(800.), px(600.)),
            },
            true,
            true,
        );
        assert!(opts.app_owns_titlebar_drag);
        assert_eq!(opts.window_decorations, Some(WindowDecorations::Client));
        assert!(opts.icon.is_some());
        assert_eq!(opts.app_id.as_deref(), Some("mio"));
        assert!(
            opts.titlebar
                .as_ref()
                .is_some_and(|t| t.appears_transparent)
        );

        let mut floating = mio_window_options(
            Bounds {
                origin: point(px(10.), px(20.)),
                size: size(px(800.), px(600.)),
            },
            true,
            true,
        );
        floating.kind = WindowKind::PopUp;
        floating.focus = false;
        floating.show = false;
        let prepared = prepare_popup_options(floating);
        assert_eq!(prepared.kind, WindowKind::Normal);
        assert!(prepared.focus);
        assert!(prepared.show);
    }
}
