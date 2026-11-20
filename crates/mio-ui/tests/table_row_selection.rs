extern crate gpui_kit as gpui;

use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    App, Context, Div, Entity, IntoElement, Render, Stateful, TestAppContext, Window, div,
    prelude::*, px, rgb,
};

struct SelectionDelegate {
    managed: bool,
}

impl TableDelegate for SelectionDelegate {
    fn columns_count(&self, _: &App) -> usize {
        1
    }

    fn rows_count(&self, _: &App) -> usize {
        3
    }

    fn column(&self, _: usize, _: &App) -> Column {
        Column::new("value", "Value").width(140.)
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        div()
            .id(("selection-row", row))
            .aria_selected(row < 2)
            .bg(rgb(0xffff64))
            .when(self.managed, |this| {
                this.hover(|this| this.bg(rgb(0xffc864)))
            })
    }

    fn row_selection_managed_by_delegate(&self) -> bool {
        self.managed
    }

    fn render_td(
        &mut self,
        row: usize,
        _: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().child(row.to_string())
    }
}

struct Harness(Entity<TableState<SelectionDelegate>>);

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(240.)).h(px(160.)).child(DataTable::new(&self.0))
    }
}

#[gpui_kit::test]
fn managed_row_selection_preserves_multiple_accessible_rows_and_hover(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    for managed in [false, true] {
        let (view, visual) = cx.add_window_view(|window, cx| {
            Harness(cx.new(|cx| TableState::new(SelectionDelegate { managed }, window, cx)))
        });
        let state = view.read_with(visual, |view, _| view.0.clone());
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("selection-row", 0usize), cx);
        });
        assert_eq!(
            state.read_with(visual, |state, _| state.selected_row()),
            Some(0)
        );
        visual.update(|window, cx| {
            window.render_frame(cx);
            for row in 0usize..3 {
                let snapshot = window.find(("selection-row", row));
                assert_eq!(snapshot.role(), Some(gpui::Role::Row));
                assert_eq!(snapshot.selected(), Some(row == 0 || managed && row == 1));
            }
            window.hover(("selection-row", 1usize), cx);
            window.render_frame(cx);
            assert_eq!(
                window.find(("selection-row", 1usize)).selected(),
                Some(managed)
            );
        });
    }
}
