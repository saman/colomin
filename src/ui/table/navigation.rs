use gpui::Context;

use super::TableView;

pub fn on_escape(view: &mut TableView, cx: &mut Context<TableView>) {
    if view.editing.is_some() {
        view.cancel_edit(cx);
        return;
    }
    let has_menu = view.state.read(cx).context_menu.is_some();
    view.state.update(cx, |s, _| {
        if has_menu {
            s.context_menu = None;
        } else {
            s.clear_selection();
        }
    });
    cx.notify();
}
