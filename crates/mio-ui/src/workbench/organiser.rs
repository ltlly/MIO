use std::collections::HashSet;

use gpui_kit::component::{
    button::ButtonVariants,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
    resizable::{resizable_panel, v_resizable},
    table::{Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState},
};
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Entity, EntityInputHandler, MouseButton, SharedString,
    Subscription, WeakEntity, Window, div, px, rgb,
};
use mio_core::{
    MAX_ORGANISER_NOTES_BYTES, OrganiserItem, OrganiserStatus, copy_url_for_request,
    next_organiser_number, validate_organiser_items,
};

use super::*;

mod actions;
mod archive;
mod detail;
mod render;
mod table;
#[cfg(test)]
mod tests;
mod transfer;

pub(in crate::workbench) use table::OrganiserTableDelegate;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum OrganiserSortColumn {
    #[default]
    Number,
    Method,
    Url,
    Status,
    Length,
    WorkflowStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) struct OrganiserSort {
    pub(in crate::workbench) column: OrganiserSortColumn,
    pub(in crate::workbench) ascending: bool,
}

impl Default for OrganiserSort {
    fn default() -> Self {
        Self {
            column: OrganiserSortColumn::Number,
            ascending: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workbench) struct OrganiserCollection {
    pub(in crate::workbench) id: mio_core::Uuid,
    pub(in crate::workbench) name: String,
    pub(in crate::workbench) item_ids: HashSet<mio_core::Uuid>,
}

pub(in crate::workbench) struct OrganiserUi {
    pub(in crate::workbench) table: Entity<TableState<OrganiserTableDelegate>>,
    pub(in crate::workbench) search: Entity<InputState>,
    pub(in crate::workbench) search_term: String,
    pub(in crate::workbench) collection_search: Entity<InputState>,
    pub(in crate::workbench) collection_search_term: String,
    pub(in crate::workbench) custom_collections: Vec<OrganiserCollection>,
    pub(in crate::workbench) active_collection: Option<mio_core::Uuid>,
    pub(in crate::workbench) creating_collection: bool,
    pub(in crate::workbench) new_collection_input: Entity<InputState>,
    pub(in crate::workbench) learn_more_open: bool,
    pub(in crate::workbench) status_filter: Option<OrganiserStatus>,
    pub(in crate::workbench) sort: OrganiserSort,
    pub(in crate::workbench) selected_id: Option<mio_core::Uuid>,
    pub(in crate::workbench) selected_ids: HashSet<mio_core::Uuid>,
    pub(in crate::workbench) selection_anchor: Option<mio_core::Uuid>,
    pub(in crate::workbench) request: MessageEditor,
    pub(in crate::workbench) response: MessageEditor,
    pub(in crate::workbench) notes: Entity<TextareaState>,
    pub(in crate::workbench) notes_open: bool,
    pub(in crate::workbench) notes_dirty: bool,
    _subscriptions: Vec<Subscription>,
}

impl OrganiserUi {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let owner = cx.weak_entity();
        let table = cx.new(|cx| {
            TableState::new(OrganiserTableDelegate::new(owner), window, cx)
                .sortable(true)
                .col_selectable(false)
                .select_on_modifier_click(false)
        });
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder("搜索 Host / URL / 方法 / 备注 / 报文预览")
        });
        let collection_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
        let new_collection_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("输入集合名称…"));
        let notes = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("输入当前报文的分析备注（支持多行中文，上限 64 KiB）…")
                .soft_wrap(true)
                .autocorrect(false)
        });
        let search_sub = cx.subscribe_in(&search, window, |this, input, event, window, cx| {
            if !matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                return;
            }
            if input.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            }) {
                return;
            }
            this.organiser.search_term = input.read(cx).value().trim().to_owned();
            this.sync_organiser_table(window, cx);
        });
        let collection_search_sub =
            cx.subscribe_in(&collection_search, window, |this, input, event, window, cx| {
                if !matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                    return;
                }
                if input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                }) {
                    return;
                }
                this.organiser.collection_search_term = input.read(cx).value().trim().to_owned();
                cx.notify();
            });
        let new_collection_sub = cx.subscribe_in(
            &new_collection_input,
            window,
            |this, input, event, window, cx| {
                if !matches!(event, InputEvent::PressEnter { .. }) {
                    return;
                }
                if input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                }) {
                    return;
                }
                this.confirm_create_organiser_collection(window, cx);
            },
        );
        let table_sub = cx.subscribe_in(&table, window, |this, _, event, window, cx| match event {
            TableEvent::SelectRow(row) | TableEvent::DoubleClickedRow(row) => {
                this.select_organiser_visual_row(*row, false, window, cx);
            }
            TableEvent::RightClickedRow(Some(row)) => {
                this.select_organiser_visual_row(*row, true, window, cx);
            }
            _ => {}
        });
        let notes_sub = cx.subscribe_in(&notes, window, |this, input, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                if input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                }) {
                    this.organiser.notes_dirty = true;
                    cx.notify();
                    return;
                }
                if let Some(id) = this.organiser.selected_id
                    && let Some(item) = this.workspace.organiser_items.iter().find(|i| i.id == id)
                {
                    this.organiser.notes_dirty =
                        input.read(cx).content().text().as_ref() != item.notes;
                    cx.notify();
                }
            }
        });
        Self {
            table,
            search,
            search_term: String::new(),
            collection_search,
            collection_search_term: String::new(),
            custom_collections: Vec::new(),
            active_collection: None,
            creating_collection: false,
            new_collection_input,
            learn_more_open: false,
            status_filter: None,
            sort: OrganiserSort::default(),
            selected_id: None,
            selected_ids: HashSet::new(),
            selection_anchor: None,
            request: MessageEditor::new(window, cx),
            response: MessageEditor::new(window, cx),
            notes,
            notes_open: true,
            notes_dirty: false,
            _subscriptions: vec![
                search_sub,
                collection_search_sub,
                new_collection_sub,
                table_sub,
                notes_sub,
            ],
        }
    }
}

impl Workbench {
    pub(in crate::workbench) fn organiser_editable(&self) -> bool {
        self.project.is_some() && self.transition.is_none() && self.backend_available()
    }

    pub(in crate::workbench) fn organiser_shows_empty_state(&self) -> bool {
        if let Some(collection_id) = self.organiser.active_collection {
            self.organiser.search_term.is_empty()
                && self
                    .organiser
                    .custom_collections
                    .iter()
                    .find(|c| c.id == collection_id)
                    .is_some_and(|c| c.item_ids.is_empty())
        } else {
            self.workspace.organiser_items.is_empty()
                && self.organiser.status_filter.is_none()
                && self.organiser.search_term.is_empty()
        }
    }

    pub(in crate::workbench) fn validate_organiser_composition(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        editing::input_snapshot(&self.organiser.collection_search, window, cx)?;
        if self.organiser.creating_collection {
            editing::input_snapshot(&self.organiser.new_collection_input, window, cx)?;
        }
        editing::input_snapshot(&self.organiser.search, window, cx)?;
        if self.organiser.notes.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) {
            return Err(editing::COMPOSITION_PENDING);
        }
        editing::input_snapshot(&self.organiser.request.search, window, cx)?;
        editing::input_snapshot(&self.organiser.response.search, window, cx)?;
        Ok(())
    }

    pub(in crate::workbench) fn reset_organiser(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.organiser.status_filter = None;
        self.organiser.active_collection = None;
        self.organiser.custom_collections.clear();
        self.organiser.creating_collection = false;
        self.organiser.learn_more_open = false;
        self.organiser.collection_search_term.clear();
        self.organiser
            .collection_search
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.organiser
            .new_collection_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.organiser.search_term.clear();
        self.organiser
            .search
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.organiser.sort = OrganiserSort::default();
        self.organiser.notes_dirty = false;
        self.organiser.selected_id = self.workspace.organiser_items.first().map(|item| item.id);
        self.organiser.selection_anchor = self.organiser.selected_id;
        self.organiser.selected_ids = self.organiser.selected_id.into_iter().collect();
        self.sync_organiser_table(window, cx);
        self.reload_selected_organiser_detail(window, cx);
    }

    pub(in crate::workbench) fn active_organiser_message_editor(
        &mut self,
        window: &Window,
        cx: &App,
    ) -> Option<&mut MessageEditor> {
        if self.tool != Tool::Organiser
            || self.organiser_shows_empty_state()
            || self.organiser.selected_id.is_none()
            || self.organiser.search.focus_handle(cx).is_focused(window)
            || self
                .organiser
                .collection_search
                .focus_handle(cx)
                .is_focused(window)
            || self
                .organiser
                .new_collection_input
                .focus_handle(cx)
                .is_focused(window)
            || self.organiser.notes.focus_handle(cx).is_focused(window)
        {
            return None;
        }
        if self.organiser.response.has_focus(window, cx) {
            Some(&mut self.organiser.response)
        } else {
            Some(&mut self.organiser.request)
        }
    }
}
