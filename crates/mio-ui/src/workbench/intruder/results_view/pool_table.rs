use gpui_kit::component::table::{Column, TableDelegate};

use super::*;

impl TableDelegate for ResultPoolTable {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        let mut col = self.columns[index].clone();
        if (scale - 1.0).abs() > f32::EPSILON {
            let width = px((col.width.as_f32() * scale).round());
            col = col.width(width);
        }
        col.paddings(gpui_kit::Edges {
            top: px(0.),
            bottom: px(0.),
            left: theme::scaled_px(8.),
            right: theme::scaled_px(8.),
        })
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.target
            .workbench
            .upgrade()
            .filter(|w| self.target.owns(w.read(cx)))
            .and_then(|w| {
                w.read(cx)
                    .intruder
                    .resource_pools
                    .catalog
                    .as_ref()
                    .map(|p| p.pools.len())
            })
            .unwrap_or(0)
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(owner) = self.target.workbench.upgrade() else {
            return div().into_any_element();
        };
        let owner = owner.read(cx);
        let Some(pool) = owner
            .intruder
            .resource_pools
            .catalog
            .as_ref()
            .and_then(|p| p.pools.get(row))
        else {
            return div().into_any_element();
        };
        if column == 0 {
            let target = self.target.clone();
            let id = pool.id;
            let checked = owner
                .intruder
                .results
                .views
                .get(&target.view_id)
                .and_then(|v| v.summary.as_ref())
                .is_some_and(|r| r.resource_pool == Some(id));
            resource_pools::pool_radio(
                SharedString::from(format!("intruder-result-pool-{id}")),
                "",
                checked,
                owner.can_change_intruder_pool(target.view_id),
            )
            .accessibility_label(pool.name.clone())
            .on_change(move |checked, _, window, cx| {
                if checked {
                    target.update(window, cx, |view, window, cx| {
                        view.change_intruder_result_pool(target.view_id, id, window, cx);
                    });
                }
            })
            .into_any_element()
        } else {
            div()
                .truncate()
                .child(resource_pools::pool_values(pool)[column - 1].clone())
                .into_any_element()
        }
    }

    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.target.workbench.upgrade()?;
        let pool = owner
            .read(cx)
            .intruder
            .resource_pools
            .catalog
            .as_ref()?
            .pools
            .get(row)?;
        Some(
            if column == 0 {
                pool.name.clone()
            } else {
                resource_pools::pool_values(pool)[column - 1].clone()
            }
            .into(),
        )
    }
}
