use std::sync::Arc;

use gpui_kit::{
    AnyElement, Bounds, ContentMask, Context, Corners, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point, RenderImage, SharedString,
    TestSupportExt, TextAlign, TextRun, accesskit::Role, canvas, div, fill, point, prelude::*, px,
};
use mio_core::panel::{Canvas, PointerPhase, Rect, draw_command};

use super::{NativePanel, theme, tone};

type Matrix = [f64; 6];
const IDENTITY: Matrix = [1., 0., 0., 1., 0., 0.];
pub(super) const PANEL_PIXEL_BUDGET: usize = 2_000_000;

#[derive(Clone, Debug)]
pub(in crate::plugin_panel) enum PreparedCommand {
    Quad {
        bounds: Bounds<Pixels>,
        radius: f32,
        tone: i32,
        clip: Option<Bounds<Pixels>>,
    },
    Polygon {
        points: [Point<Pixels>; 4],
        tone: i32,
        clip: Option<Bounds<Pixels>>,
    },
    Path {
        points: Vec<Point<Pixels>>,
        closed: bool,
        filled: bool,
        stroke_width: f32,
        tone: i32,
        clip: Option<Bounds<Pixels>>,
    },
    Text {
        origin: Point<Pixels>,
        text: SharedString,
        font_size: f32,
        tone: i32,
        clip: Option<Bounds<Pixels>>,
    },
    Image {
        bounds: Bounds<Pixels>,
        image_index: usize,
        clip: Option<Bounds<Pixels>>,
    },
}

#[derive(Clone, Debug)]
pub(in crate::plugin_panel) struct PreparedCanvas {
    pub(in crate::plugin_panel) commands: Vec<PreparedCommand>,
    pub(in crate::plugin_panel) image_count: usize,
}

fn multiply(a: Matrix, b: Matrix) -> Result<Matrix, String> {
    let result = [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ];
    if result.iter().any(|value| !(*value as f32).is_finite()) {
        return Err("Canvas 合成变换超过可绘制范围。".into());
    }
    Ok(result)
}

fn transform_point(matrix: Matrix, x: f32, y: f32) -> Result<Point<Pixels>, String> {
    let x = f64::from(x);
    let y = f64::from(y);
    let tx = (matrix[0] * x + matrix[2] * y + matrix[4]) as f32;
    let ty = (matrix[1] * x + matrix[3] * y + matrix[5]) as f32;
    if !tx.is_finite() || !ty.is_finite() {
        return Err("Canvas 坐标变换超过可绘制范围。".into());
    }
    Ok(point(px(tx), px(ty)))
}

fn transform_rect_bounds(matrix: Matrix, rect: &Rect) -> Result<Bounds<Pixels>, String> {
    let corners = [
        transform_point(matrix, rect.x, rect.y)?,
        transform_point(matrix, rect.x + rect.width, rect.y)?,
        transform_point(matrix, rect.x + rect.width, rect.y + rect.height)?,
        transform_point(matrix, rect.x, rect.y + rect.height)?,
    ];
    let min_x = corners
        .iter()
        .map(|pt| pt.x.as_f32())
        .fold(f32::INFINITY, f32::min);
    let max_x = corners
        .iter()
        .map(|pt| pt.x.as_f32())
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = corners
        .iter()
        .map(|pt| pt.y.as_f32())
        .fold(f32::INFINITY, f32::min);
    let max_y = corners
        .iter()
        .map(|pt| pt.y.as_f32())
        .fold(f32::NEG_INFINITY, f32::max);
    Ok(Bounds::from_corners(
        point(px(min_x), px(min_y)),
        point(px(max_x), px(max_y)),
    ))
}

fn intersect_bounds(a: Bounds<Pixels>, b: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    let left = a.origin.x.max(b.origin.x);
    let top = a.origin.y.max(b.origin.y);
    let right = (a.origin.x + a.size.width).min(b.origin.x + b.size.width);
    let bottom = (a.origin.y + a.size.height).min(b.origin.y + b.size.height);
    (right > left && bottom > top).then(|| {
        Bounds::from_corners(point(left, top), point(right, bottom))
    })
}

fn active_clip(clips: &[Bounds<Pixels>]) -> Option<Option<Bounds<Pixels>>> {
    if clips.is_empty() {
        return Some(None);
    }
    let mut current = clips[0];
    for &next in &clips[1..] {
        current = intersect_bounds(current, next)?;
    }
    Some(Some(current))
}

fn offset_point(pt: Point<Pixels>, origin: Point<Pixels>) -> Point<Pixels> {
    point(origin.x + pt.x, origin.y + pt.y)
}

fn offset_bounds(bounds: Bounds<Pixels>, origin: Point<Pixels>) -> Bounds<Pixels> {
    Bounds {
        origin: point(origin.x + bounds.origin.x, origin.y + bounds.origin.y),
        size: bounds.size,
    }
}

pub(in crate::plugin_panel) fn validate_image_budget(
    canvas: &Canvas,
    pixel_budget: usize,
) -> Result<usize, String> {
    canvas.validate()?;
    // Sharing the existing raster allowance caps intrinsic PNG RGBA pixels at 8M per panel.
    let image_pixel_budget = pixel_budget.min(PANEL_PIXEL_BUDGET) * 4;
    let mut image_pixels = 0usize;
    let mut image_count = 0usize;
    for command in &canvas.commands {
        let Some(draw_command::Op::Image(image)) = &command.op else {
            continue;
        };
        image_count += 1;
        let pixels = (image.pixel_width as usize)
            .checked_mul(image.pixel_height as usize)
            .ok_or("Canvas 图片解码总像素溢出；未生成图像。")?;
        image_pixels = image_pixels
            .checked_add(pixels)
            .ok_or("Canvas 图片解码总像素溢出；未生成图像。")?;
        if image_pixels > image_pixel_budget {
            return Err(format!(
                "Canvas 图片解码总像素 {image_pixels} 超过面板分配的 {image_pixel_budget} 像素预算；未生成图像。"
            ));
        }
    }
    Ok(image_count)
}

pub(in crate::plugin_panel) fn prepare_canvas(
    canvas: &Canvas,
    pixel_budget: usize,
) -> Result<PreparedCanvas, String> {
    let image_count = validate_image_budget(canvas, pixel_budget)?;
    let mut prepared = Vec::with_capacity(canvas.commands.len());
    let mut matrices = vec![IDENTITY];
    let mut clips: Vec<Bounds<Pixels>> = Vec::new();
    let mut next_image_index = 0usize;

    for command in &canvas.commands {
        let matrix = *matrices.last().expect("identity matrix remains");
        let operation = command.op.as_ref().expect("validated Canvas operation");
        match operation {
            draw_command::Op::PushTransform(value) => {
                let incoming: Matrix = std::array::from_fn(|index| value.matrix[index] as f64);
                matrices.push(multiply(matrix, incoming)?);
                continue;
            }
            draw_command::Op::PopTransform(_) => {
                matrices.pop();
                continue;
            }
            draw_command::Op::PushClip(bounds) => {
                // Capture the transform at clip creation; clip and transform stacks are independent.
                clips.push(transform_rect_bounds(matrix, bounds)?);
                continue;
            }
            draw_command::Op::PopClip(_) => {
                clips.pop();
                continue;
            }
            _ => {}
        }

        let maybe_clip = active_clip(&clips);
        if let draw_command::Op::Image(value) = operation {
            let image_index = next_image_index;
            next_image_index += 1;
            if let Some(clip) = maybe_clip {
                let bounds = transform_rect_bounds(matrix, value.bounds.as_ref().unwrap())?;
                prepared.push(PreparedCommand::Image {
                    bounds,
                    image_index,
                    clip,
                });
            }
            continue;
        }
        let Some(clip) = maybe_clip else {
            continue;
        };

        match operation {
            draw_command::Op::Rect(value) => {
                let rect = value.bounds.as_ref().unwrap();
                if matrix[1] == 0.0 && matrix[2] == 0.0 {
                    let bounds = transform_rect_bounds(matrix, rect)?;
                    let radius_scale = matrix[0].abs().min(matrix[3].abs()) as f32;
                    let radius = (value.radius * radius_scale).max(0.0);
                    prepared.push(PreparedCommand::Quad {
                        bounds,
                        radius,
                        tone: value.tone,
                        clip,
                    });
                } else {
                    let points = [
                        transform_point(matrix, rect.x, rect.y)?,
                        transform_point(matrix, rect.x + rect.width, rect.y)?,
                        transform_point(matrix, rect.x + rect.width, rect.y + rect.height)?,
                        transform_point(matrix, rect.x, rect.y + rect.height)?,
                    ];
                    prepared.push(PreparedCommand::Polygon {
                        points,
                        tone: value.tone,
                        clip,
                    });
                }
            }
            draw_command::Op::Path(value) => {
                let mut points = Vec::with_capacity(value.points.len());
                for pt in &value.points {
                    points.push(transform_point(matrix, pt.x, pt.y)?);
                }
                let scale = ((matrix[0].hypot(matrix[1]) + matrix[2].hypot(matrix[3])) * 0.5) as f32;
                let stroke_width = if value.stroke_width > 0.0 {
                    (value.stroke_width * scale).max(0.5)
                } else {
                    0.0
                };
                prepared.push(PreparedCommand::Path {
                    points,
                    closed: value.closed,
                    filled: value.filled,
                    stroke_width,
                    tone: value.tone,
                    clip,
                });
            }
            draw_command::Op::Text(value) => {
                let origin = value.origin.as_ref().unwrap();
                let transformed_origin = transform_point(matrix, origin.x, origin.y)?;
                let scale = matrix[2].hypot(matrix[3]) as f32;
                let font_size = if scale > 0.0 && scale.is_finite() {
                    (value.size * scale).max(1.0)
                } else {
                    value.size
                };
                prepared.push(PreparedCommand::Text {
                    origin: transformed_origin,
                    text: SharedString::from(value.text.clone()),
                    font_size,
                    tone: value.tone,
                    clip,
                });
            }
            _ => unreachable!("stack and image operations handled before drawing"),
        }
    }

    Ok(PreparedCanvas {
        commands: prepared,
        image_count,
    })
}

pub(in crate::plugin_panel) fn decode_canvas_images(
    canvas: &Canvas,
    pixel_budget: usize,
) -> Result<Vec<Arc<RenderImage>>, String> {
    let image_count = validate_image_budget(canvas, pixel_budget)?;
    let mut images = Vec::with_capacity(image_count);
    for command in &canvas.commands {
        let Some(draw_command::Op::Image(value)) = &command.op else {
            continue;
        };
        let decoded = image::load_from_memory_with_format(&value.png, image::ImageFormat::Png)
            .map_err(|error| format!("Canvas 图片解码失败：{error}"))?;
        if decoded.width() != value.pixel_width || decoded.height() != value.pixel_height {
            return Err("Canvas PNG 尺寸与声明像素宽高不一致。".into());
        }
        let mut rgba = decoded.into_rgba8();
        // GPUI RenderImage expects premultiplied/pixel layout in BGRA byte order.
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        images.push(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])));
    }
    Ok(images)
}

impl NativePanel {
    pub(in crate::plugin_panel) fn render_canvas(
        &self,
        id: u64,
        model: &mio_core::panel::Canvas,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let drawing = &self.drawings[&id];
        let bounds = drawing.bounds.clone();
        let prepared = drawing.prepared.clone();
        let images = drawing.images.clone();
        let awaiting_images = drawing
            .prepared
            .as_ref()
            .is_some_and(|p| p.image_count > 0 && drawing.images.is_empty());

        let mut area = div()
            .id(("plugin-canvas", id))
            .test_support()
            .relative()
            .w(px(model.width))
            .h(px(model.height))
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.pointer_down(id, event.position, window, cx)
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                this.hover_pointer(id, event.position, PointerPhase::Move, cx)
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                    this.hover_pointer(id, event.position, PointerPhase::Up, cx)
                }),
            )
            .child(
                canvas(
                    move |geometry, _, _| {
                        bounds.set(geometry);
                    },
                    move |canvas_bounds, _, window, cx| {
                        let Some(prepared) = prepared.as_ref() else {
                            return;
                        };
                        let origin = canvas_bounds.origin;
                        for command in &prepared.commands {
                            let clip = match command {
                                PreparedCommand::Quad { clip, .. }
                                | PreparedCommand::Polygon { clip, .. }
                                | PreparedCommand::Path { clip, .. }
                                | PreparedCommand::Text { clip, .. }
                                | PreparedCommand::Image { clip, .. } => *clip,
                            };
                            let mask = match clip {
                                Some(local_clip) => {
                                    let Some(masked) = intersect_bounds(
                                        canvas_bounds,
                                        offset_bounds(local_clip, origin),
                                    ) else {
                                        continue;
                                    };
                                    masked
                                }
                                None => canvas_bounds,
                            };
                            window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
                                match command {
                                    PreparedCommand::Quad {
                                        bounds,
                                        radius,
                                        tone: tone_val,
                                        ..
                                    } => {
                                        let color = tone(*tone_val);
                                        let quad_bounds = offset_bounds(*bounds, origin);
                                        window.paint_quad(
                                            fill(quad_bounds, color).corner_radii(px(*radius)),
                                        );
                                    }
                                    PreparedCommand::Polygon {
                                        points,
                                        tone: tone_val,
                                        ..
                                    } => {
                                        let color = tone(*tone_val);
                                        let mut builder = PathBuilder::fill();
                                        builder.move_to(offset_point(points[0], origin));
                                        for &pt in &points[1..] {
                                            builder.line_to(offset_point(pt, origin));
                                        }
                                        builder.close();
                                        if let Ok(path) = builder.build() {
                                            window.paint_path(path, color);
                                        }
                                    }
                                    PreparedCommand::Path {
                                        points,
                                        closed,
                                        filled,
                                        stroke_width,
                                        tone: tone_val,
                                        ..
                                    } => {
                                        if points.len() < 2 {
                                            return;
                                        }
                                        let color = tone(*tone_val);
                                        let mut builder = if *filled {
                                            PathBuilder::fill()
                                        } else {
                                            PathBuilder::stroke(px(*stroke_width))
                                        };
                                        builder.move_to(offset_point(points[0], origin));
                                        for &pt in &points[1..] {
                                            builder.line_to(offset_point(pt, origin));
                                        }
                                        if *closed {
                                            builder.close();
                                        }
                                        if let Ok(path) = builder.build() {
                                            window.paint_path(path, color);
                                        }
                                    }
                                    PreparedCommand::Text {
                                        origin: text_origin,
                                        text,
                                        font_size,
                                        tone: tone_val,
                                        ..
                                    } => {
                                        if text.is_empty() {
                                            return;
                                        }
                                        let color = tone(*tone_val);
                                        let font = window.text_style().font();
                                        let size = px(*font_size);
                                        let line_height = size * 1.25;
                                        let base = offset_point(*text_origin, origin);
                                        for (line_idx, segment) in text.split('\n').enumerate() {
                                            if segment.is_empty() {
                                                continue;
                                            }
                                            let run = TextRun {
                                                len: segment.len(),
                                                font: font.clone(),
                                                color,
                                                background_color: None,
                                                underline: None,
                                                strikethrough: None,
                                            };
                                            let line = window.text_system().shape_line(
                                                SharedString::from(segment.to_owned()),
                                                size,
                                                &[run],
                                                None,
                                            );
                                            let line_origin = point(
                                                base.x,
                                                base.y + line_height * (line_idx as f32),
                                            );
                                            let _ = line.paint(
                                                line_origin,
                                                size,
                                                TextAlign::Left,
                                                None,
                                                window,
                                                cx,
                                            );
                                        }
                                    }
                                    PreparedCommand::Image {
                                        bounds,
                                        image_index,
                                        ..
                                    } => {
                                        if let Some(image) = images.get(*image_index) {
                                            let image_bounds = offset_bounds(*bounds, origin);
                                            let _ = window.paint_image(
                                                image_bounds,
                                                image_bounds,
                                                Corners::default(),
                                                image.clone(),
                                                0,
                                                false,
                                            );
                                        }
                                    }
                                }
                            });
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        if drawing.error.is_some() || drawing.prepared.is_none() || awaiting_images {
            area = area.child(
                div()
                    .text_color(if drawing.error.is_some() {
                        theme::danger_fg()
                    } else {
                        theme::muted_foreground()
                    })
                    .child(
                        drawing
                            .error
                            .clone()
                            .unwrap_or_else(|| "正在绘制 Canvas…".into()),
                    ),
            );
        }
        for hit in &model.hit_regions {
            let rect = hit.bounds.as_ref().unwrap();
            let mut region = div()
                .id(("plugin-hit", hit.id))
                .test_support()
                .role(Role::Group)
                .aria_label(hit.accessibility_label.clone())
                .absolute()
                .left(px(rect.x))
                .top(px(rect.y))
                .w(px(rect.width))
                .h(px(rect.height));
            if let Some(focus) = drawing.focus.get(&hit.id) {
                region = region
                    .track_focus(&focus.handle)
                    .tab_index(0)
                    .focus_visible(|style| style.border_1().border_color(theme::accent()))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                        this.key(id, event, cx)
                    }));
            }
            area = area.child(region);
        }
        area.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt;
    use mio_core::panel::{
        DrawCommand, DrawImage, DrawRect, DrawText, Empty, Node, PanelSnapshot, Point, Tone,
        Transform, node,
    };

    fn image_canvas(dimensions: &[(u32, u32)]) -> Canvas {
        Canvas {
            width: 100.,
            height: 100.,
            hit_regions: vec![],
            commands: dimensions
                .iter()
                .map(|&(width, height)| {
                    // Only the tiny IHDR declaration is needed: these tests never decode PNGs.
                    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
                    png.extend_from_slice(&width.to_be_bytes());
                    png.extend_from_slice(&height.to_be_bytes());
                    DrawCommand {
                        op: Some(draw_command::Op::Image(DrawImage {
                            bounds: Some(Rect {
                                x: 0.,
                                y: 0.,
                                width: 100.,
                                height: 100.,
                            }),
                            pixel_width: width,
                            pixel_height: height,
                            png,
                        })),
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn png_decode_budget_rejects_aggregate_before_expansion() {
        let canvas = image_canvas(&[(2000, 2000), (2000, 2000), (1, 1)]);
        canvas.validate().unwrap();
        assert!(
            prepare_canvas(&canvas, PANEL_PIXEL_BUDGET)
                .is_err_and(|error| error.contains("图片解码总像素")),
            "individually valid compressed images must not exceed the aggregate decode budget"
        );
    }

    #[test]
    fn png_decode_budget_accepts_the_exact_boundary_without_decoding() {
        let canvas = image_canvas(&[(2000, 2000), (2000, 2000)]);
        assert!(prepare_canvas(&canvas, PANEL_PIXEL_BUDGET).is_ok());
    }

    #[test]
    fn png_decode_budget_is_shared_by_all_visible_canvases() {
        let canvas = image_canvas(&[(2000, 2000)]);
        assert!(prepare_canvas(&canvas, PANEL_PIXEL_BUDGET).is_ok());
        // prepare_drawings divides one panel's raster budget among its visible canvases.
        let per_canvas_budget = PANEL_PIXEL_BUDGET / 3;
        for _ in 0..3 {
            assert!(
                prepare_canvas(&canvas, per_canvas_budget)
                    .is_err_and(|error| error.contains("图片解码总像素")),
                "adding canvases must not multiply the panel's image decode allowance"
            );
        }
        assert!(prepare_canvas(&image_canvas(&[(1000, 1000)]), per_canvas_budget).is_ok());
    }

    #[test]
    fn native_canvas_keeps_raw_text_and_independent_stacks_keep_their_own_state() {
        let bounds = Rect {
            x: 0.,
            y: 0.,
            width: 20.,
            height: 20.,
        };
        let canvas = Canvas {
            width: 100.,
            height: 100.,
            hit_regions: vec![],
            commands: vec![
                DrawCommand {
                    op: Some(draw_command::Op::PushClip(bounds.clone())),
                },
                DrawCommand {
                    op: Some(draw_command::Op::PushTransform(Transform {
                        matrix: vec![1., 0., 0., 1., 10., 5.],
                    })),
                },
                DrawCommand {
                    op: Some(draw_command::Op::PopClip(Empty {})),
                },
                DrawCommand {
                    op: Some(draw_command::Op::Rect(DrawRect {
                        bounds: Some(bounds),
                        tone: Tone::Accent as i32,
                        radius: 2.,
                    })),
                },
                DrawCommand {
                    op: Some(draw_command::Op::PopTransform(Empty {})),
                },
                DrawCommand {
                    op: Some(draw_command::Op::Text(DrawText {
                        origin: Some(Point { x: 0., y: 0. }),
                        text: "<image href='file:///x'/> & \"中文\"".into(),
                        tone: Tone::Foreground as i32,
                        size: 13.,
                    })),
                },
            ],
        };
        let prepared = prepare_canvas(&canvas, PANEL_PIXEL_BUDGET).unwrap();
        assert_eq!(prepared.commands.len(), 2);
        match &prepared.commands[0] {
            PreparedCommand::Quad {
                bounds,
                radius,
                tone,
                clip,
            } => {
                assert_eq!(bounds.origin, point(px(10.), px(5.)));
                assert_eq!(bounds.size, gpui_kit::size(px(20.), px(20.)));
                assert_eq!(*radius, 2.);
                assert_eq!(*tone, Tone::Accent as i32);
                assert!(clip.is_none());
            }
            other => panic!("expected Quad, got {other:?}"),
        }
        match &prepared.commands[1] {
            PreparedCommand::Text {
                origin,
                text,
                font_size,
                clip,
                ..
            } => {
                assert_eq!(*origin, point(px(0.), px(0.)));
                assert_eq!(text.as_ref(), "<image href='file:///x'/> & \"中文\"");
                assert_eq!(*font_size, 13.);
                assert!(clip.is_none());
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn malformed_canvas_is_rejected_before_indexing_or_rasterization() {
        let canvas = Canvas {
            width: f32::NAN,
            height: 10.,
            ..Default::default()
        };
        assert!(prepare_canvas(&canvas, PANEL_PIXEL_BUDGET).is_err());
        assert!(multiply([f32::MAX as f64; 6], [f32::MAX as f64; 6]).is_err());
    }

    #[gpui_kit::test]
    fn native_canvas_paints_theme_aware_quads_for_light_and_dark_modes(cx: &mut TestAppContext) {
        let snapshot = PanelSnapshot {
            revision: 1,
            title: "Themed Canvas".into(),
            root: Some(Node::new(
                42,
                "Canvas",
                node::Kind::Canvas(Canvas {
                    width: 120.,
                    height: 80.,
                    hit_regions: vec![],
                    commands: vec![DrawCommand {
                        op: Some(draw_command::Op::Rect(DrawRect {
                            bounds: Some(Rect {
                                x: 10.,
                                y: 12.,
                                width: 40.,
                                height: 24.,
                            }),
                            tone: Tone::Foreground as i32,
                            radius: 3.,
                        })),
                    }],
                }),
            )),
        };
        let fixture = crate::plugin_panel::tests::fixture(cx, snapshot);
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let light_fg = theme::foreground();
            assert!(!theme::is_dark());
            assert!(
                window
                    .painted_quads()
                    .iter()
                    .any(|quad| quad.background == light_fg.into()),
                "light mode canvas rect must paint light theme::foreground()"
            );

            theme::apply_user_display(
                &mio_core::UserDisplaySettings {
                    theme: mio_core::UserThemeMode::Dark,
                    ..mio_core::UserDisplaySettings::default()
                },
                &mio_core::UserMessageEditorSettings::default(),
                cx,
            );
            window.render_frame(cx);
            let dark_fg = theme::foreground();
            assert_ne!(dark_fg, light_fg);
            assert!(
                window
                    .painted_quads()
                    .iter()
                    .any(|quad| quad.background == dark_fg.into()),
                "dark mode canvas rect must repaint with dark theme::foreground()"
            );

            theme::init(cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn native_canvas_paints_multiline_draw_text_without_panicking(cx: &mut TestAppContext) {
        let snapshot = PanelSnapshot {
            revision: 1,
            title: "Multiline Text Canvas".into(),
            root: Some(Node::new(
                43,
                "Canvas",
                node::Kind::Canvas(Canvas {
                    width: 160.,
                    height: 120.,
                    hit_regions: vec![],
                    commands: vec![DrawCommand {
                        op: Some(draw_command::Op::Text(DrawText {
                            origin: Some(Point { x: 4., y: 6. }),
                            text: "line one\n\nline three".into(),
                            tone: Tone::Foreground as i32,
                            size: 12.,
                        })),
                    }],
                }),
            )),
        };
        let fixture = crate::plugin_panel::tests::fixture(cx, snapshot);
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
    }

    #[test]
    fn decode_canvas_images_decodes_png_to_bgra_and_rejects_mismatched_dimensions() {
        let mut rgba = image::RgbaImage::new(2, 1);
        // Pixel 0: opaque red [R=255, G=10, B=20, A=255] -> BGRA [20, 10, 255, 255]
        // Pixel 1: semi-transparent blue [R=1, G=2, B=200, A=128] -> BGRA [200, 2, 1, 128]
        rgba.put_pixel(0, 0, image::Rgba([255, 10, 20, 255]));
        rgba.put_pixel(1, 0, image::Rgba([1, 2, 200, 128]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let canvas = Canvas {
            width: 100.,
            height: 100.,
            hit_regions: vec![],
            commands: vec![DrawCommand {
                op: Some(draw_command::Op::Image(DrawImage {
                    bounds: Some(Rect {
                        x: 0.,
                        y: 0.,
                        width: 20.,
                        height: 10.,
                    }),
                    pixel_width: 2,
                    pixel_height: 1,
                    png: png.clone(),
                })),
            }],
        };
        let decoded = decode_canvas_images(&canvas, PANEL_PIXEL_BUDGET).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].size(0), gpui_kit::size(gpui_kit::DevicePixels(2), gpui_kit::DevicePixels(1)));
        assert_eq!(
            decoded[0].as_bytes(0).unwrap(),
            &[20, 10, 255, 255, 200, 2, 1, 128]
        );

        let mismatched = Canvas {
            width: 100.,
            height: 100.,
            hit_regions: vec![],
            commands: vec![DrawCommand {
                op: Some(draw_command::Op::Image(DrawImage {
                    bounds: Some(Rect {
                        x: 0.,
                        y: 0.,
                        width: 20.,
                        height: 10.,
                    }),
                    pixel_width: 4,
                    pixel_height: 1,
                    png,
                })),
            }],
        };
        assert!(decode_canvas_images(&mismatched, PANEL_PIXEL_BUDGET).is_err());

        let mut corrupt = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        corrupt.extend_from_slice(&2u32.to_be_bytes());
        corrupt.extend_from_slice(&1u32.to_be_bytes());
        corrupt.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        let corrupted = Canvas {
            width: 100.,
            height: 100.,
            hit_regions: vec![],
            commands: vec![DrawCommand {
                op: Some(draw_command::Op::Image(DrawImage {
                    bounds: Some(Rect {
                        x: 0.,
                        y: 0.,
                        width: 20.,
                        height: 10.,
                    }),
                    pixel_width: 2,
                    pixel_height: 1,
                    png: corrupt,
                })),
            }],
        };
        assert!(
            decode_canvas_images(&corrupted, PANEL_PIXEL_BUDGET)
                .is_err_and(|error| error.contains("Canvas 图片解码失败"))
        );
    }
}
