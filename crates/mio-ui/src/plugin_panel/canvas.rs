use std::fmt::Write;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use mio_core::panel::{Canvas, Rect, Tone, draw_command};

use super::*;

type Matrix = [f64; 6];
const IDENTITY: Matrix = [1., 0., 0., 1., 0., 0.];
pub(super) const PANEL_PIXEL_BUDGET: usize = 2_000_000;

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

fn transform(matrix: Matrix) -> String {
    format!(
        "matrix({} {} {} {} {} {})",
        matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5]
    )
}

fn rect(rect: &Rect) -> String {
    format!(
        "x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"",
        rect.x, rect.y, rect.width, rect.height
    )
}

pub(super) fn tone(value: i32) -> &'static str {
    match Tone::try_from(value).expect("validated Canvas tone") {
        Tone::Foreground => "#222222",
        Tone::Muted => "#6b6b6b",
        Tone::Accent => "#ff6633",
        Tone::Error => "#b42318",
        Tone::Background => "#f4f4f4",
        Tone::Border => "#c7c7c7",
        Tone::Selection => "#d6e8fc",
    }
}

fn xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            '\t'
            | '\n'
            | '\r'
            | '\u{20}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'..='\u{10ffff}' => escaped.push(character),
            _ => escaped.push('\u{fffd}'),
        }
    }
    escaped
}

/// Only typed, validated drawing operations enter this generated SVG. It never accepts an SVG,
/// external URL, file reference, script, or event handler supplied by a plugin.
pub(super) fn svg(canvas: &Canvas, pixel_budget: usize) -> Result<Vec<u8>, String> {
    canvas.validate()?;
    // Sharing the existing raster allowance also caps intrinsic PNG RGBA pixels at 8M per panel.
    let image_pixel_budget = pixel_budget.min(PANEL_PIXEL_BUDGET) * 4;
    let mut image_pixels = 0usize;
    for command in &canvas.commands {
        let Some(draw_command::Op::Image(image)) = &command.op else {
            continue;
        };
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
    // Bound raster memory while retaining the whole logical canvas and every drawing command.
    let scale = (2048. / canvas.width.max(canvas.height))
        .min((pixel_budget.max(1) as f32 / (canvas.width * canvas.height)).sqrt())
        .min(1.);
    let mut output = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\"><defs>",
        (canvas.width * scale).max(1.),
        (canvas.height * scale).max(1.),
        canvas.width,
        canvas.height
    );
    let mut drawing = String::new();
    let mut matrices = vec![IDENTITY];
    let mut clips = Vec::new();
    let mut next_clip = 0usize;
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
                // Capture the transform at clip creation; the clip and transform stacks are independent.
                write!(output, "<clipPath id=\"clip{next_clip}\" clipPathUnits=\"userSpaceOnUse\"><rect {} transform=\"{}\"/></clipPath>", rect(bounds), transform(matrix)).unwrap();
                clips.push(next_clip);
                next_clip += 1;
                continue;
            }
            draw_command::Op::PopClip(_) => {
                clips.pop();
                continue;
            }
            _ => {}
        }
        for clip in &clips {
            write!(drawing, "<g clip-path=\"url(#clip{clip})\">").unwrap();
        }
        write!(drawing, "<g transform=\"{}\">", transform(matrix)).unwrap();
        match operation {
            draw_command::Op::Rect(value) => {
                write!(
                    drawing,
                    "<rect {} rx=\"{}\" fill=\"{}\"/>",
                    rect(value.bounds.as_ref().unwrap()),
                    value.radius,
                    tone(value.tone)
                )
                .unwrap();
            }
            draw_command::Op::Path(value) => {
                let tag = if value.closed { "polygon" } else { "polyline" };
                write!(drawing, "<{tag} points=\"").unwrap();
                for point in &value.points {
                    write!(drawing, "{},{} ", point.x, point.y).unwrap();
                }
                write!(
                    drawing,
                    "\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>",
                    if value.filled {
                        tone(value.tone)
                    } else {
                        "none"
                    },
                    if value.filled {
                        "none"
                    } else {
                        tone(value.tone)
                    },
                    value.stroke_width
                )
                .unwrap();
            }
            draw_command::Op::Text(value) => {
                let origin = value.origin.as_ref().unwrap();
                write!(drawing, "<text x=\"{}\" y=\"{}\" dominant-baseline=\"text-before-edge\" font-family=\"sans-serif\" font-size=\"{}\" fill=\"{}\" xml:space=\"preserve\">{}</text>", origin.x, origin.y, value.size, tone(value.tone), xml(&value.text)).unwrap();
            }
            draw_command::Op::Image(value) => {
                write!(drawing, "<image {} preserveAspectRatio=\"none\" xlink:href=\"data:image/png;base64,{}\"/>", rect(value.bounds.as_ref().unwrap()), STANDARD.encode(&value.png)).unwrap();
            }
            _ => unreachable!("stack operations handled before drawing"),
        }
        drawing.push_str("</g>");
        for _ in &clips {
            drawing.push_str("</g>");
        }
    }
    output.push_str("</defs>");
    output.push_str(&drawing);
    output.push_str("</svg>");
    Ok(output.into_bytes())
}

impl NativePanel {
    pub(in crate::plugin_panel) fn render_canvas(
        &self,
        id: u64,
        model: &panel::Canvas,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let drawing = &self.drawings[&id];
        let bounds = drawing.bounds.clone();
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
                canvas(move |geometry, _, _| bounds.set(geometry), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            );
        if let Some(image) = &drawing.image {
            area = area.child(img(image.clone()).absolute().size_full());
        }
        if drawing.error.is_some() || drawing.image.is_none() {
            area = area.child(
                div()
                    .text_color(if drawing.error.is_some() {
                        rgb(0xb42318)
                    } else {
                        rgb(0x6b6b6b)
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
    use mio_core::panel::{DrawCommand, DrawImage, DrawRect, DrawText, Empty, Point, Transform};

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
    fn png_decode_budget_rejects_aggregate_before_svg_expansion() {
        let canvas = image_canvas(&[(2000, 2000), (2000, 2000), (1, 1)]);
        canvas.validate().unwrap();
        assert!(
            svg(&canvas, PANEL_PIXEL_BUDGET).is_err_and(|error| error.contains("图片解码总像素")),
            "individually valid compressed images must not exceed the aggregate decode budget"
        );
    }

    #[test]
    fn png_decode_budget_accepts_the_exact_boundary_without_decoding() {
        let canvas = image_canvas(&[(2000, 2000), (2000, 2000)]);
        assert!(svg(&canvas, PANEL_PIXEL_BUDGET).is_ok());
    }

    #[test]
    fn png_decode_budget_is_shared_by_all_visible_canvases() {
        let canvas = image_canvas(&[(2000, 2000)]);
        assert!(svg(&canvas, PANEL_PIXEL_BUDGET).is_ok());
        // prepare_drawings divides one panel's raster budget among its visible canvases.
        let per_canvas_budget = PANEL_PIXEL_BUDGET / 3;
        for _ in 0..3 {
            assert!(
                svg(&canvas, per_canvas_budget)
                    .is_err_and(|error| error.contains("图片解码总像素")),
                "adding canvases must not multiply the panel's image decode allowance"
            );
        }
        assert!(svg(&image_canvas(&[(1000, 1000)]), per_canvas_budget).is_ok());
    }

    #[test]
    fn generated_svg_escapes_text_and_independent_stacks_keep_their_own_state() {
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
        let svg = String::from_utf8(svg(&canvas, 2_000_000).unwrap()).unwrap();
        assert!(svg.contains("transform=\"matrix(1 0 0 1 10 5)\"><rect"));
        assert!(!svg.contains("<g clip-path="));
        assert!(!svg.contains("<image href="));
        assert!(svg.contains("&lt;image href=&apos;file:///x&apos;/&gt; &amp; &quot;中文&quot;"));
    }

    #[test]
    fn malformed_canvas_is_rejected_before_indexing_or_rasterization() {
        let canvas = Canvas {
            width: f32::NAN,
            height: 10.,
            ..Default::default()
        };
        assert!(svg(&canvas, 2_000_000).is_err());
        assert!(multiply([f32::MAX as f64; 6], [f32::MAX as f64; 6]).is_err());
    }
}
