use client::{Contact, UserStore};
use gpui::{
    action,
    elements::*,
    geometry::{rect::RectF, vector::vec2f},
    platform::CursorStyle,
    Element, ElementBox, Entity, LayoutContext, ModelHandle, MutableAppContext, RenderContext,
    Subscription, View, ViewContext,
};
use postage::watch;
use theme::Theme;
use workspace::{Settings, Workspace};

action!(JoinWorktree, u64);
action!(LeaveWorktree, u64);
action!(ShareWorktree, u64);
action!(UnshareWorktree, u64);

pub fn init(cx: &mut MutableAppContext) {
    cx.add_action(ContactsPanel::share_worktree);
    cx.add_action(ContactsPanel::unshare_worktree);
    cx.add_action(ContactsPanel::join_worktree);
    cx.add_action(ContactsPanel::leave_worktree);
}

pub struct ContactsPanel {
    contacts: ListState,
    user_store: ModelHandle<UserStore>,
    settings: watch::Receiver<Settings>,
    _maintain_contacts: Subscription,
}

impl ContactsPanel {
    pub fn new(
        user_store: ModelHandle<UserStore>,
        settings: watch::Receiver<Settings>,
        cx: &mut ViewContext<Self>,
    ) -> Self {
        Self {
            contacts: ListState::new(
                user_store.read(cx).contacts().len(),
                Orientation::Top,
                1000.,
                {
                    let user_store = user_store.clone();
                    let settings = settings.clone();
                    move |ix, cx| {
                        let user_store = user_store.read(cx);
                        let contacts = user_store.contacts().clone();
                        let current_user_id = user_store.current_user().map(|user| user.id);
                        Self::render_collaborator(
                            &contacts[ix],
                            current_user_id,
                            &settings.borrow().theme,
                            cx,
                        )
                    }
                },
            ),
            _maintain_contacts: cx.observe(&user_store, Self::update_contacts),
            user_store,
            settings,
        }
    }

    fn share_worktree(
        workspace: &mut Workspace,
        action: &ShareWorktree,
        cx: &mut ViewContext<Workspace>,
    ) {
        workspace
            .project()
            .update(cx, |p, cx| p.share_worktree(action.0, cx));
    }

    fn unshare_worktree(
        workspace: &mut Workspace,
        action: &UnshareWorktree,
        cx: &mut ViewContext<Workspace>,
    ) {
        workspace
            .project()
            .update(cx, |p, cx| p.unshare_worktree(action.0, cx));
    }

    fn join_worktree(
        workspace: &mut Workspace,
        action: &JoinWorktree,
        cx: &mut ViewContext<Workspace>,
    ) {
        workspace
            .project()
            .update(cx, |p, cx| p.add_remote_worktree(action.0, cx).detach());
    }

    fn leave_worktree(
        workspace: &mut Workspace,
        action: &LeaveWorktree,
        cx: &mut ViewContext<Workspace>,
    ) {
        workspace
            .project()
            .update(cx, |p, cx| p.close_remote_worktree(action.0, cx));
    }

    fn update_contacts(&mut self, _: ModelHandle<UserStore>, cx: &mut ViewContext<Self>) {
        self.contacts
            .reset(self.user_store.read(cx).contacts().len());
        cx.notify();
    }

    fn render_collaborator(
        collaborator: &Contact,
        current_user_id: Option<u64>,
        theme: &Theme,
        cx: &mut LayoutContext,
    ) -> ElementBox {
        let theme = &theme.contacts_panel;
        let worktree_count = collaborator.worktrees.len();
        let font_cache = cx.font_cache();
        let line_height = theme.unshared_worktree.name.text.line_height(font_cache);
        let cap_height = theme.unshared_worktree.name.text.cap_height(font_cache);
        let baseline_offset = theme
            .unshared_worktree
            .name
            .text
            .baseline_offset(font_cache)
            + (theme.unshared_worktree.height - line_height) / 2.;
        let tree_branch_width = theme.tree_branch_width;
        let tree_branch_color = theme.tree_branch_color;
        let host_avatar_height = theme
            .host_avatar
            .width
            .or(theme.host_avatar.height)
            .unwrap_or(0.);

        Flex::column()
            .with_child(
                Flex::row()
                    .with_children(collaborator.user.avatar.clone().map(|avatar| {
                        Image::new(avatar)
                            .with_style(theme.host_avatar)
                            .aligned()
                            .left()
                            .boxed()
                    }))
                    .with_child(
                        Label::new(
                            collaborator.user.github_login.clone(),
                            theme.host_username.text.clone(),
                        )
                        .contained()
                        .with_style(theme.host_username.container)
                        .aligned()
                        .left()
                        .boxed(),
                    )
                    .constrained()
                    .with_height(theme.host_row_height)
                    .boxed(),
            )
            .with_children(
                collaborator
                    .worktrees
                    .iter()
                    .enumerate()
                    .map(|(ix, worktree)| {
                        let worktree_id = worktree.id;

                        Flex::row()
                            .with_child(
                                Canvas::new(move |bounds, _, cx| {
                                    let start_x = bounds.min_x() + (bounds.width() / 2.)
                                        - (tree_branch_width / 2.);
                                    let end_x = bounds.max_x();
                                    let start_y = bounds.min_y();
                                    let end_y =
                                        bounds.min_y() + baseline_offset - (cap_height / 2.);

                                    cx.scene.push_quad(gpui::Quad {
                                        bounds: RectF::from_points(
                                            vec2f(start_x, start_y),
                                            vec2f(
                                                start_x + tree_branch_width,
                                                if ix + 1 == worktree_count {
                                                    end_y
                                                } else {
                                                    bounds.max_y()
                                                },
                                            ),
                                        ),
                                        background: Some(tree_branch_color),
                                        border: gpui::Border::default(),
                                        corner_radius: 0.,
                                        checkerboard: Default::default(),
                                    });
                                    cx.scene.push_quad(gpui::Quad {
                                        bounds: RectF::from_points(
                                            vec2f(start_x, end_y),
                                            vec2f(end_x, end_y + tree_branch_width),
                                        ),
                                        background: Some(tree_branch_color),
                                        border: gpui::Border::default(),
                                        corner_radius: 0.,
                                        checkerboard: Default::default(),
                                    });
                                })
                                .constrained()
                                .with_width(host_avatar_height)
                                .boxed(),
                            )
                            .with_child({
                                let is_host = Some(collaborator.user.id) == current_user_id;
                                let is_guest = !is_host
                                    && worktree
                                        .guests
                                        .iter()
                                        .any(|guest| Some(guest.id) == current_user_id);
                                let is_shared = worktree.is_shared;

                                MouseEventHandler::new::<ContactsPanel, _, _, _>(
                                    worktree_id as usize,
                                    cx,
                                    |mouse_state, _| {
                                        let style = match (worktree.is_shared, mouse_state.hovered)
                                        {
                                            (false, false) => &theme.unshared_worktree,
                                            (false, true) => &theme.hovered_unshared_worktree,
                                            (true, false) => &theme.shared_worktree,
                                            (true, true) => &theme.hovered_shared_worktree,
                                        };

                                        Flex::row()
                                            .with_child(
                                                Label::new(
                                                    worktree.root_name.clone(),
                                                    style.name.text.clone(),
                                                )
                                                .aligned()
                                                .left()
                                                .contained()
                                                .with_style(style.name.container)
                                                .boxed(),
                                            )
                                            .with_children(worktree.guests.iter().filter_map(
                                                |participant| {
                                                    participant.avatar.clone().map(|avatar| {
                                                        Image::new(avatar)
                                                            .with_style(style.guest_avatar)
                                                            .aligned()
                                                            .left()
                                                            .contained()
                                                            .with_margin_right(
                                                                style.guest_avatar_spacing,
                                                            )
                                                            .boxed()
                                                    })
                                                },
                                            ))
                                            .contained()
                                            .with_style(style.container)
                                            .constrained()
                                            .with_height(style.height)
                                            .boxed()
                                    },
                                )
                                .with_cursor_style(if is_host || is_shared {
                                    CursorStyle::PointingHand
                                } else {
                                    CursorStyle::Arrow
                                })
                                .on_click(move |cx| {
                                    if is_shared {
                                        if is_host {
                                            cx.dispatch_action(UnshareWorktree(worktree_id));
                                        } else if is_guest {
                                            cx.dispatch_action(LeaveWorktree(worktree_id));
                                        } else {
                                            cx.dispatch_action(JoinWorktree(worktree_id))
                                        }
                                    } else if is_host {
                                        cx.dispatch_action(ShareWorktree(worktree_id));
                                    }
                                })
                                .expanded(1.0)
                                .boxed()
                            })
                            .constrained()
                            .with_height(theme.unshared_worktree.height)
                            .boxed()
                    }),
            )
            .boxed()
    }
}

pub enum Event {}

impl Entity for ContactsPanel {
    type Event = Event;
}

impl View for ContactsPanel {
    fn ui_name() -> &'static str {
        "ContactsPanel"
    }

    fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
        let theme = &self.settings.borrow().theme.contacts_panel;
        Container::new(List::new(self.contacts.clone()).boxed())
            .with_style(theme.container)
            .boxed()
    }
}
