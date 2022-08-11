use crate::StatusItemView;
use gpui::{
    elements::*, impl_actions, platform::CursorStyle, AnyViewHandle, AppContext, Entity,
    MouseButton, RenderContext, Subscription, View, ViewContext, ViewHandle,
};
use serde::Deserialize;
use settings::Settings;
use std::{cell::RefCell, rc::Rc};
use theme::Theme;

pub trait SidebarItem: View {
    fn should_activate_item_on_event(&self, _: &Self::Event, _: &AppContext) -> bool {
        false
    }
    fn should_show_badge(&self, cx: &AppContext) -> bool;
    fn contains_focused_view(&self, _: &AppContext) -> bool {
        false
    }
}

pub trait SidebarItemHandle {
    fn id(&self) -> usize;
    fn should_show_badge(&self, cx: &AppContext) -> bool;
    fn is_focused(&self, cx: &AppContext) -> bool;
    fn to_any(&self) -> AnyViewHandle;
}

impl<T> SidebarItemHandle for ViewHandle<T>
where
    T: SidebarItem,
{
    fn id(&self) -> usize {
        self.id()
    }

    fn should_show_badge(&self, cx: &AppContext) -> bool {
        self.read(cx).should_show_badge(cx)
    }

    fn is_focused(&self, cx: &AppContext) -> bool {
        ViewHandle::is_focused(self, cx) || self.read(cx).contains_focused_view(cx)
    }

    fn to_any(&self) -> AnyViewHandle {
        self.into()
    }
}

impl From<&dyn SidebarItemHandle> for AnyViewHandle {
    fn from(val: &dyn SidebarItemHandle) -> Self {
        val.to_any()
    }
}

pub struct Sidebar {
    side: Side,
    items: Vec<Item>,
    is_open: bool,
    active_item_ix: usize,
    actual_width: Rc<RefCell<f32>>,
    custom_width: Rc<RefCell<f32>>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub enum Side {
    Left,
    Right,
}

struct Item {
    icon_path: &'static str,
    tooltip: String,
    view: Rc<dyn SidebarItemHandle>,
    _subscriptions: [Subscription; 2],
}

pub struct SidebarButtons {
    sidebar: ViewHandle<Sidebar>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ToggleSidebarItem {
    pub side: Side,
    pub item_index: usize,
}

impl_actions!(workspace, [ToggleSidebarItem]);

impl Sidebar {
    pub fn new(side: Side) -> Self {
        Self {
            side,
            items: Default::default(),
            active_item_ix: 0,
            is_open: false,
            actual_width: Rc::new(RefCell::new(260.)),
            custom_width: Rc::new(RefCell::new(260.)),
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn active_item_ix(&self) -> usize {
        self.active_item_ix
    }

    pub fn set_open(&mut self, open: bool, cx: &mut ViewContext<Self>) {
        if open != self.is_open {
            self.is_open = open;
            cx.notify();
        }
    }

    pub fn toggle_open(&mut self, cx: &mut ViewContext<Self>) {
        if self.is_open {}
        self.is_open = !self.is_open;
        cx.notify();
    }

    pub fn add_item<T: SidebarItem>(
        &mut self,
        icon_path: &'static str,
        tooltip: String,
        view: ViewHandle<T>,
        cx: &mut ViewContext<Self>,
    ) {
        let subscriptions = [
            cx.observe(&view, |_, _, cx| cx.notify()),
            cx.subscribe(&view, |this, view, event, cx| {
                if view.read(cx).should_activate_item_on_event(event, cx) {
                    if let Some(ix) = this
                        .items
                        .iter()
                        .position(|item| item.view.id() == view.id())
                    {
                        this.activate_item(ix, cx);
                    }
                }
            }),
        ];
        cx.reparent(&view);
        self.items.push(Item {
            icon_path,
            tooltip,
            view: Rc::new(view),
            _subscriptions: subscriptions,
        });
        cx.notify()
    }

    pub fn activate_item(&mut self, item_ix: usize, cx: &mut ViewContext<Self>) {
        self.active_item_ix = item_ix;
        cx.notify();
    }

    pub fn toggle_item(&mut self, item_ix: usize, cx: &mut ViewContext<Self>) {
        if self.active_item_ix == item_ix {
            self.is_open = false;
        } else {
            self.active_item_ix = item_ix;
        }
        cx.notify();
    }

    pub fn active_item(&self) -> Option<&Rc<dyn SidebarItemHandle>> {
        if self.is_open {
            self.items.get(self.active_item_ix).map(|item| &item.view)
        } else {
            None
        }
    }

    fn render_resize_handle(&self, theme: &Theme, cx: &mut RenderContext<Self>) -> ElementBox {
        let actual_width = self.actual_width.clone();
        let custom_width = self.custom_width.clone();
        let side = self.side;
        MouseEventHandler::new::<Self, _, _>(side as usize, cx, |_, _| {
            Empty::new()
                .contained()
                .with_style(theme.workspace.sidebar_resize_handle)
                .boxed()
        })
        .with_padding(Padding {
            left: 4.,
            right: 4.,
            ..Default::default()
        })
        .with_cursor_style(CursorStyle::ResizeLeftRight)
        .on_down(MouseButton::Left, |_, _| {}) // This prevents the mouse down event from being propagated elsewhere
        .on_drag(MouseButton::Left, move |e, cx| {
            let delta = e.prev_drag_position.x() - e.position.x();
            let prev_width = *actual_width.borrow();
            *custom_width.borrow_mut() = 0f32
                .max(match side {
                    Side::Left => prev_width + delta,
                    Side::Right => prev_width - delta,
                })
                .round();

            cx.notify();
        })
        .boxed()
    }
}

impl Entity for Sidebar {
    type Event = ();
}

impl View for Sidebar {
    fn ui_name() -> &'static str {
        "Sidebar"
    }

    fn render(&mut self, cx: &mut RenderContext<Self>) -> ElementBox {
        let theme = cx.global::<Settings>().theme.clone();
        if let Some(active_item) = self.active_item() {
            let mut container = Flex::row();
            if matches!(self.side, Side::Right) {
                container.add_child(self.render_resize_handle(&theme, cx));
            }

            container.add_child(
                Hook::new(
                    ChildView::new(active_item.to_any())
                        .constrained()
                        .with_max_width(*self.custom_width.borrow())
                        .boxed(),
                )
                .on_after_layout({
                    let actual_width = self.actual_width.clone();
                    move |size, _| *actual_width.borrow_mut() = size.x()
                })
                .flex(1., false)
                .boxed(),
            );
            if matches!(self.side, Side::Left) {
                container.add_child(self.render_resize_handle(&theme, cx));
            }
            container.boxed()
        } else {
            Empty::new().boxed()
        }
    }
}

impl SidebarButtons {
    pub fn new(sidebar: ViewHandle<Sidebar>, cx: &mut ViewContext<Self>) -> Self {
        cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
        Self { sidebar }
    }
}

impl Entity for SidebarButtons {
    type Event = ();
}

impl View for SidebarButtons {
    fn ui_name() -> &'static str {
        "SidebarToggleButton"
    }

    fn render(&mut self, cx: &mut RenderContext<Self>) -> ElementBox {
        let theme = &cx.global::<Settings>().theme;
        let tooltip_style = theme.tooltip.clone();
        let theme = &theme.workspace.status_bar.sidebar_buttons;
        let sidebar = self.sidebar.read(cx);
        let item_style = theme.item;
        let badge_style = theme.badge;
        let active_ix = sidebar.active_item_ix;
        let is_open = sidebar.is_open;
        let side = sidebar.side;
        let group_style = match side {
            Side::Left => theme.group_left,
            Side::Right => theme.group_right,
        };

        #[allow(clippy::needless_collect)]
        let items = sidebar
            .items
            .iter()
            .map(|item| (item.icon_path, item.tooltip.clone(), item.view.clone()))
            .collect::<Vec<_>>();

        Flex::row()
            .with_children(items.into_iter().enumerate().map(
                |(ix, (icon_path, tooltip, item_view))| {
                    let action = ToggleSidebarItem {
                        side,
                        item_index: ix,
                    };
                    MouseEventHandler::new::<Self, _, _>(ix, cx, move |state, cx| {
                        let is_active = is_open && ix == active_ix;
                        let style = item_style.style_for(state, is_active);
                        Stack::new()
                            .with_child(Svg::new(icon_path).with_color(style.icon_color).boxed())
                            .with_children(if !is_active && item_view.should_show_badge(cx) {
                                Some(
                                    Empty::new()
                                        .collapsed()
                                        .contained()
                                        .with_style(badge_style)
                                        .aligned()
                                        .bottom()
                                        .right()
                                        .boxed(),
                                )
                            } else {
                                None
                            })
                            .constrained()
                            .with_width(style.icon_size)
                            .with_height(style.icon_size)
                            .contained()
                            .with_style(style.container)
                            .boxed()
                    })
                    .with_cursor_style(CursorStyle::PointingHand)
                    .on_click(MouseButton::Left, {
                        let action = action.clone();
                        move |_, cx| cx.dispatch_action(action.clone())
                    })
                    .with_tooltip::<Self, _>(
                        ix,
                        tooltip,
                        Some(Box::new(action)),
                        tooltip_style.clone(),
                        cx,
                    )
                    .boxed()
                },
            ))
            .contained()
            .with_style(group_style)
            .boxed()
    }
}

impl StatusItemView for SidebarButtons {
    fn set_active_pane_item(
        &mut self,
        _: Option<&dyn crate::ItemHandle>,
        _: &mut ViewContext<Self>,
    ) {
    }
}
