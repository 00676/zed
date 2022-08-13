use crate::{
    app::{AppContext, MutableAppContext, WindowInvalidation},
    elements::Element,
    font_cache::FontCache,
    geometry::rect::RectF,
    json::{self, ToJson},
    keymap::Keystroke,
    platform::{CursorStyle, Event},
    scene::{
        ClickRegionEvent, CursorRegion, DownOutRegionEvent, DownRegionEvent, DragRegionEvent,
        HoverRegionEvent, MouseRegionEvent, MoveRegionEvent, RegionEventMaker, UpOutRegionEvent,
        UpRegionEvent,
    },
    text_layout::TextLayoutCache,
    Action, AnyModelHandle, AnyViewHandle, AnyWeakModelHandle, AssetCache, ElementBox, Entity,
    FontSystem, ModelHandle, MouseButton, MouseButtonEvent, MouseMovedEvent, MouseRegion,
    MouseRegionId, ParentId, ReadModel, ReadView, RenderContext, RenderParams, Scene,
    UpgradeModelHandle, UpgradeViewHandle, View, ViewHandle, WeakModelHandle, WeakViewHandle,
};
use collections::{HashMap, HashSet};
use pathfinder_geometry::vector::{vec2f, Vector2F};
use serde_json::json;
use smallvec::SmallVec;
use std::{
    any::TypeId,
    marker::PhantomData,
    ops::{Deref, DerefMut, Range},
    sync::Arc,
};

pub struct Presenter {
    window_id: usize,
    pub(crate) rendered_views: HashMap<usize, ElementBox>,
    cursor_regions: Vec<CursorRegion>,
    mouse_regions: Vec<(MouseRegion, usize)>,
    font_cache: Arc<FontCache>,
    text_layout_cache: TextLayoutCache,
    asset_cache: Arc<AssetCache>,
    last_mouse_moved_event: Option<Event>,
    hovered_region_ids: HashSet<MouseRegionId>,
    clicked_regions: Vec<MouseRegion>,
    clicked_button: Option<MouseButton>,
    mouse_position: Vector2F,
    titlebar_height: f32,
}

impl Presenter {
    pub fn new(
        window_id: usize,
        titlebar_height: f32,
        font_cache: Arc<FontCache>,
        text_layout_cache: TextLayoutCache,
        asset_cache: Arc<AssetCache>,
        cx: &mut MutableAppContext,
    ) -> Self {
        Self {
            window_id,
            rendered_views: cx.render_views(window_id, titlebar_height),
            cursor_regions: Default::default(),
            mouse_regions: Default::default(),
            font_cache,
            text_layout_cache,
            asset_cache,
            last_mouse_moved_event: None,
            hovered_region_ids: Default::default(),
            clicked_regions: Vec::new(),
            clicked_button: None,
            mouse_position: vec2f(0., 0.),
            titlebar_height,
        }
    }

    pub fn invalidate(
        &mut self,
        invalidation: &mut WindowInvalidation,
        cx: &mut MutableAppContext,
    ) {
        cx.start_frame();
        for view_id in &invalidation.removed {
            invalidation.updated.remove(view_id);
            self.rendered_views.remove(view_id);
        }
        for view_id in &invalidation.updated {
            self.rendered_views.insert(
                *view_id,
                cx.render_view(RenderParams {
                    window_id: self.window_id,
                    view_id: *view_id,
                    titlebar_height: self.titlebar_height,
                    hovered_region_ids: self.hovered_region_ids.clone(),
                    clicked_region_ids: self.clicked_button.map(|button| {
                        (
                            self.clicked_regions
                                .iter()
                                .filter_map(MouseRegion::id)
                                .collect(),
                            button,
                        )
                    }),
                    refreshing: false,
                })
                .unwrap(),
            );
        }
    }

    pub fn refresh(&mut self, invalidation: &mut WindowInvalidation, cx: &mut MutableAppContext) {
        self.invalidate(invalidation, cx);
        for (view_id, view) in &mut self.rendered_views {
            if !invalidation.updated.contains(view_id) {
                *view = cx
                    .render_view(RenderParams {
                        window_id: self.window_id,
                        view_id: *view_id,
                        titlebar_height: self.titlebar_height,
                        hovered_region_ids: self.hovered_region_ids.clone(),
                        clicked_region_ids: self.clicked_button.map(|button| {
                            (
                                self.clicked_regions
                                    .iter()
                                    .filter_map(MouseRegion::id)
                                    .collect(),
                                button,
                            )
                        }),
                        refreshing: true,
                    })
                    .unwrap();
            }
        }
    }

    pub fn build_scene(
        &mut self,
        window_size: Vector2F,
        scale_factor: f32,
        refreshing: bool,
        cx: &mut MutableAppContext,
    ) -> Scene {
        let mut scene = Scene::new(scale_factor);

        if let Some(root_view_id) = cx.root_view_id(self.window_id) {
            self.layout(window_size, refreshing, cx);
            let mut paint_cx = self.build_paint_context(&mut scene, window_size, cx);
            paint_cx.paint(
                root_view_id,
                Vector2F::zero(),
                RectF::new(Vector2F::zero(), window_size),
            );
            self.text_layout_cache.finish_frame();
            self.cursor_regions = scene.cursor_regions();
            self.mouse_regions = scene.mouse_regions();

            if cx.window_is_active(self.window_id) {
                if let Some(event) = self.last_mouse_moved_event.clone() {
                    self.dispatch_event(event, true, cx);
                }
            }
        } else {
            log::error!("could not find root_view_id for window {}", self.window_id);
        }

        scene
    }

    fn layout(&mut self, window_size: Vector2F, refreshing: bool, cx: &mut MutableAppContext) {
        if let Some(root_view_id) = cx.root_view_id(self.window_id) {
            self.build_layout_context(window_size, refreshing, cx)
                .layout(root_view_id, SizeConstraint::strict(window_size));
        }
    }

    pub fn build_layout_context<'a>(
        &'a mut self,
        window_size: Vector2F,
        refreshing: bool,
        cx: &'a mut MutableAppContext,
    ) -> LayoutContext<'a> {
        LayoutContext {
            window_id: self.window_id,
            rendered_views: &mut self.rendered_views,
            font_cache: &self.font_cache,
            font_system: cx.platform().fonts(),
            text_layout_cache: &self.text_layout_cache,
            asset_cache: &self.asset_cache,
            view_stack: Vec::new(),
            refreshing,
            hovered_region_ids: self.hovered_region_ids.clone(),
            clicked_region_ids: self.clicked_button.map(|button| {
                (
                    self.clicked_regions
                        .iter()
                        .filter_map(MouseRegion::id)
                        .collect(),
                    button,
                )
            }),
            titlebar_height: self.titlebar_height,
            window_size,
            app: cx,
        }
    }

    pub fn build_paint_context<'a>(
        &'a mut self,
        scene: &'a mut Scene,
        window_size: Vector2F,
        cx: &'a mut MutableAppContext,
    ) -> PaintContext {
        PaintContext {
            scene,
            window_size,
            font_cache: &self.font_cache,
            text_layout_cache: &self.text_layout_cache,
            rendered_views: &mut self.rendered_views,
            view_stack: Vec::new(),
            app: cx,
        }
    }

    pub fn rect_for_text_range(&self, range_utf16: Range<usize>, cx: &AppContext) -> Option<RectF> {
        cx.focused_view_id(self.window_id).and_then(|view_id| {
            let cx = MeasurementContext {
                app: cx,
                rendered_views: &self.rendered_views,
                window_id: self.window_id,
            };
            cx.rect_for_text_range(view_id, range_utf16)
        })
    }

    pub fn make_hover_events(
        &mut self,
        e: &MouseMovedEvent,
    ) -> Vec<(MouseRegion, MouseRegionEvent)> {
        let mut res = vec![];

        //GPUI elements are arranged by depth but sibling elements can register overlapping
        //mouse regions. As such, hover events are only fired on overlapping elements which
        //are at the same depth as the deepest element which overlaps with the mouse.
        let mut top_most_depth = None;
        let mouse_position = self.mouse_position.clone();
        for (region, depth) in self.mouse_regions.iter().rev() {
            let contains_mouse = region.bounds.contains_point(mouse_position);

            if contains_mouse && top_most_depth.is_none() {
                top_most_depth = Some(depth);
            }

            if let Some(region_id) = region.id() {
                //This unwrap relies on short circuiting boolean expressions
                //The right side of the && is only executed when contains_mouse
                //is true, and we know above that when contains_mouse is true
                //top_most_depth is set
                if contains_mouse && depth == top_most_depth.unwrap() {
                    //Ensure that hover entrance events aren't sent twice
                    if self.hovered_region_ids.insert(region_id) {
                        res.push((
                            region.clone(),
                            MouseRegionEvent::Hover(HoverRegionEvent {
                                region: region.bounds,
                                platform_event: e.clone(),
                                started: true,
                            }),
                        ));
                    }
                } else {
                    //Ensure that hover exit events aren't sent twice
                    if self.hovered_region_ids.remove(&region_id) {
                        res.push((
                            region.clone(),
                            MouseRegionEvent::Hover(HoverRegionEvent {
                                region: region.bounds,
                                platform_event: e.clone(),
                                started: false,
                            }),
                        ));
                    }
                }
            }
        }

        res
    }

    fn make_click_events(&mut self, e: &MouseButtonEvent) -> Vec<(MouseRegion, MouseRegionEvent)> {
        let mut res = vec![];

        //Clear presenter state
        let clicked_regions = std::mem::replace(&mut self.clicked_regions, Vec::new());
        self.clicked_button = None;

        //Find regions which still overlap with the mouse since the last MouseDown happened
        for clicked_region in clicked_regions.into_iter().rev() {
            if clicked_region.bounds.contains_point(e.position) {
                res.push((
                    clicked_region.clone(),
                    MouseRegionEvent::Click(ClickRegionEvent {
                        region: clicked_region.bounds,
                        platform_event: e.clone(),
                    }),
                ));
            }
        }

        res
    }

    fn make_drag_events(&mut self, e: &MouseMovedEvent) -> Vec<(MouseRegion, MouseRegionEvent)> {
        let mut res = vec![];

        for (mouse_region, _) in self.mouse_regions.iter().rev() {
            if mouse_region.bounds.contains_point(self.mouse_position) {
                res.push((
                    mouse_region.clone(),
                    MouseRegionEvent::Drag(DragRegionEvent {
                        region: mouse_region.bounds,
                        prev_mouse_position: self.mouse_position,
                        platform_event: e.clone(),
                    }),
                ));
            }
        }

        res
    }

    fn make_events<E, R: RegionEventMaker<E>>(
        &mut self,
        e: &E,
        local: bool,
    ) -> Vec<(MouseRegion, MouseRegionEvent)> {
        let mut res = vec![];

        for (mouse_region, _) in self.mouse_regions.iter().rev() {
            let contains = mouse_region.bounds.contains_point(self.mouse_position);
            if local {
                if contains {
                    res.push((mouse_region.clone(), R::make(mouse_region, e)));
                }
            } else {
                if !contains {
                    res.push((mouse_region.clone(), R::make(mouse_region, e)));
                }
            }
        }

        res
    }

    pub fn dispatch_event(
        &mut self,
        event: Event,
        _event_reused: bool,
        cx: &mut MutableAppContext,
    ) -> bool {
        let mut region_map: HashMap<TypeId, Vec<(MouseRegion, MouseRegionEvent)>> =
            Default::default();
        let mut event_types: Vec<TypeId> = vec![];

        if let Some(root_view_id) = cx.root_view_id(self.window_id) {
            //1. Allocate the correct set of GPUI events generated from the platform events
            // -> These are usually small: [Mouse Down] or [Mouse up, Click] or [Mouse Moved, Mouse Dragged?]
            // -> Also moves around mouse related state
            match &event {
                Event::MouseDown(e) => {
                    //Click events are weird because they can be fired after a drag event.
                    //MDN says that browsers handle this by starting from 'the most
                    //specific ancestor element that contained both [positions]'
                    //So we need to store the overlapping regions on mouse down.
                    self.clicked_regions = self
                        .mouse_regions
                        .iter()
                        .filter_map(|(region, _)| {
                            region
                                .bounds
                                .contains_point(e.position)
                                .then(|| region.clone())
                        })
                        .collect();
                    self.clicked_button = Some(e.button);

                    event_types.push(TypeId::of::<DownRegionEvent>());
                    region_map.insert(
                        TypeId::of::<DownRegionEvent>(),
                        self.make_events::<MouseButtonEvent, DownRegionEvent>(e, true),
                    );

                    event_types.push(TypeId::of::<DownOutRegionEvent>());
                    region_map.insert(
                        TypeId::of::<DownOutRegionEvent>(),
                        self.make_events::<MouseButtonEvent, DownOutRegionEvent>(e, false),
                    );
                }
                Event::MouseUp(e) => {
                    //NOTE: The order of events here is important! MouseUp events MUST be fired
                    //before click events

                    event_types.push(TypeId::of::<UpRegionEvent>());
                    region_map.insert(
                        TypeId::of::<UpRegionEvent>(),
                        self.make_events::<MouseButtonEvent, UpRegionEvent>(e, true),
                    );

                    event_types.push(TypeId::of::<UpOutRegionEvent>());
                    region_map.insert(
                        TypeId::of::<UpOutRegionEvent>(),
                        self.make_events::<MouseButtonEvent, UpOutRegionEvent>(e, false),
                    );

                    event_types.push(TypeId::of::<ClickRegionEvent>());
                    region_map.insert(TypeId::of::<ClickRegionEvent>(), self.make_click_events(e));
                }
                Event::MouseMoved(
                    e @ MouseMovedEvent {
                        position,
                        pressed_button,
                        ..
                    },
                ) => {
                    let mut style_to_assign = CursorStyle::Arrow;
                    for region in self.cursor_regions.iter().rev() {
                        if region.bounds.contains_point(*position) {
                            style_to_assign = region.style;
                            break;
                        }
                    }
                    cx.platform().set_cursor_style(style_to_assign);

                    if pressed_button.is_some() {
                        event_types.push(TypeId::of::<DragRegionEvent>());
                        region_map
                            .insert(TypeId::of::<DragRegionEvent>(), self.make_drag_events(e));
                    }

                    event_types.push(TypeId::of::<MoveRegionEvent>());
                    region_map.insert(
                        TypeId::of::<MoveRegionEvent>(),
                        self.make_events::<MouseMovedEvent, MoveRegionEvent>(e, true),
                    );

                    event_types.push(TypeId::of::<HoverRegionEvent>());
                    region_map.insert(TypeId::of::<HoverRegionEvent>(), self.make_hover_events(e));

                    self.last_mouse_moved_event = Some(event.clone());
                }
                _ => {}
            }

            if let Some(position) = event.position() {
                self.mouse_position = position;
            }

            let mut event_cx = self.build_event_context(cx);
            let mut any_event_handled = false;
            //2. Process the raw mouse events into region events
            for event_type in event_types {
                let valid_regions = region_map.get(&event_type).unwrap(); //TODO

                //3. Fire region events
                for (valid_region, region_event) in valid_regions.into_iter() {
                    if let Some(callback) = valid_region.handlers.get(&region_event.handler_key()) {
                        event_cx.handled = true;
                        let local = region_event.is_local();
                        event_cx.with_current_view(valid_region.view_id, {
                            let region_event = region_event.clone();
                            |cx| {
                                callback(region_event, cx);
                            }
                        });

                        // For bubbling events, if the event was handled, don't continue dispatching
                        // This only makes sense for local events. 'Out*' events are already
                        if event_cx.handled && local {
                            break;
                        }
                    }
                }

                any_event_handled = any_event_handled && event_cx.handled;
            }

            if !any_event_handled {
                any_event_handled = event_cx.dispatch_event(root_view_id, &event);
            }

            for view_id in event_cx.invalidated_views {
                cx.notify_view(self.window_id, view_id);
            }

            any_event_handled
        } else {
            false
        }
    }

    pub fn build_event_context<'a>(
        &'a mut self,
        cx: &'a mut MutableAppContext,
    ) -> EventContext<'a> {
        EventContext {
            rendered_views: &mut self.rendered_views,
            font_cache: &self.font_cache,
            text_layout_cache: &self.text_layout_cache,
            view_stack: Default::default(),
            invalidated_views: Default::default(),
            notify_count: 0,
            handled: false,
            window_id: self.window_id,
            app: cx,
        }
    }

    pub fn debug_elements(&self, cx: &AppContext) -> Option<json::Value> {
        let view = cx.root_view(self.window_id)?;
        Some(json!({
            "root_view": view.debug_json(cx),
            "root_element": self.rendered_views.get(&view.id())
                .map(|root_element| {
                    root_element.debug(&DebugContext {
                        rendered_views: &self.rendered_views,
                        font_cache: &self.font_cache,
                        app: cx,
                    })
                })
        }))
    }
}

pub struct LayoutContext<'a> {
    window_id: usize,
    rendered_views: &'a mut HashMap<usize, ElementBox>,
    view_stack: Vec<usize>,
    pub font_cache: &'a Arc<FontCache>,
    pub font_system: Arc<dyn FontSystem>,
    pub text_layout_cache: &'a TextLayoutCache,
    pub asset_cache: &'a AssetCache,
    pub app: &'a mut MutableAppContext,
    pub refreshing: bool,
    pub window_size: Vector2F,
    titlebar_height: f32,
    hovered_region_ids: HashSet<MouseRegionId>,
    clicked_region_ids: Option<(Vec<MouseRegionId>, MouseButton)>,
}

impl<'a> LayoutContext<'a> {
    pub(crate) fn keystrokes_for_action(
        &self,
        action: &dyn Action,
    ) -> Option<SmallVec<[Keystroke; 2]>> {
        self.app
            .keystrokes_for_action(self.window_id, &self.view_stack, action)
    }

    fn layout(&mut self, view_id: usize, constraint: SizeConstraint) -> Vector2F {
        let print_error = |view_id| {
            format!(
                "{} with id {}",
                self.app.name_for_view(self.window_id, view_id).unwrap(),
                view_id,
            )
        };
        match (
            self.view_stack.last(),
            self.app.parents.get(&(self.window_id, view_id)),
        ) {
            (Some(layout_parent), Some(ParentId::View(app_parent))) => {
                if layout_parent != app_parent {
                    panic!(
                        "View {} was laid out with parent {} when it was constructed with parent {}", 
                        print_error(view_id),
                        print_error(*layout_parent),
                        print_error(*app_parent))
                }
            }
            (None, Some(ParentId::View(app_parent))) => panic!(
                "View {} was laid out without a parent when it was constructed with parent {}",
                print_error(view_id),
                print_error(*app_parent)
            ),
            (Some(layout_parent), Some(ParentId::Root)) => panic!(
                "View {} was laid out with parent {} when it was constructed as a window root",
                print_error(view_id),
                print_error(*layout_parent),
            ),
            (_, None) => panic!(
                "View {} did not have a registered parent in the app context",
                print_error(view_id),
            ),
            _ => {}
        }

        self.view_stack.push(view_id);
        let mut rendered_view = self.rendered_views.remove(&view_id).unwrap();
        let size = rendered_view.layout(constraint, self);
        self.rendered_views.insert(view_id, rendered_view);
        self.view_stack.pop();
        size
    }

    pub fn render<F, V, T>(&mut self, handle: &ViewHandle<V>, f: F) -> T
    where
        F: FnOnce(&mut V, &mut RenderContext<V>) -> T,
        V: View,
    {
        handle.update(self.app, |view, cx| {
            let mut render_cx = RenderContext {
                app: cx,
                window_id: handle.window_id(),
                view_id: handle.id(),
                view_type: PhantomData,
                titlebar_height: self.titlebar_height,
                hovered_region_ids: self.hovered_region_ids.clone(),
                clicked_region_ids: self.clicked_region_ids.clone(),
                refreshing: self.refreshing,
            };
            f(view, &mut render_cx)
        })
    }
}

impl<'a> Deref for LayoutContext<'a> {
    type Target = MutableAppContext;

    fn deref(&self) -> &Self::Target {
        self.app
    }
}

impl<'a> DerefMut for LayoutContext<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.app
    }
}

impl<'a> ReadView for LayoutContext<'a> {
    fn read_view<T: View>(&self, handle: &ViewHandle<T>) -> &T {
        self.app.read_view(handle)
    }
}

impl<'a> ReadModel for LayoutContext<'a> {
    fn read_model<T: Entity>(&self, handle: &ModelHandle<T>) -> &T {
        self.app.read_model(handle)
    }
}

impl<'a> UpgradeModelHandle for LayoutContext<'a> {
    fn upgrade_model_handle<T: Entity>(
        &self,
        handle: &WeakModelHandle<T>,
    ) -> Option<ModelHandle<T>> {
        self.app.upgrade_model_handle(handle)
    }

    fn model_handle_is_upgradable<T: Entity>(&self, handle: &WeakModelHandle<T>) -> bool {
        self.app.model_handle_is_upgradable(handle)
    }

    fn upgrade_any_model_handle(&self, handle: &AnyWeakModelHandle) -> Option<AnyModelHandle> {
        self.app.upgrade_any_model_handle(handle)
    }
}

impl<'a> UpgradeViewHandle for LayoutContext<'a> {
    fn upgrade_view_handle<T: View>(&self, handle: &WeakViewHandle<T>) -> Option<ViewHandle<T>> {
        self.app.upgrade_view_handle(handle)
    }

    fn upgrade_any_view_handle(&self, handle: &crate::AnyWeakViewHandle) -> Option<AnyViewHandle> {
        self.app.upgrade_any_view_handle(handle)
    }
}

pub struct PaintContext<'a> {
    rendered_views: &'a mut HashMap<usize, ElementBox>,
    view_stack: Vec<usize>,
    pub window_size: Vector2F,
    pub scene: &'a mut Scene,
    pub font_cache: &'a FontCache,
    pub text_layout_cache: &'a TextLayoutCache,
    pub app: &'a AppContext,
}

impl<'a> PaintContext<'a> {
    fn paint(&mut self, view_id: usize, origin: Vector2F, visible_bounds: RectF) {
        if let Some(mut tree) = self.rendered_views.remove(&view_id) {
            self.view_stack.push(view_id);
            tree.paint(origin, visible_bounds, self);
            self.rendered_views.insert(view_id, tree);
            self.view_stack.pop();
        }
    }

    #[inline]
    pub fn paint_layer<F>(&mut self, clip_bounds: Option<RectF>, f: F)
    where
        F: FnOnce(&mut Self),
    {
        self.scene.push_layer(clip_bounds);
        f(self);
        self.scene.pop_layer();
    }

    pub fn current_view_id(&self) -> usize {
        *self.view_stack.last().unwrap()
    }
}

impl<'a> Deref for PaintContext<'a> {
    type Target = AppContext;

    fn deref(&self) -> &Self::Target {
        self.app
    }
}

pub struct EventContext<'a> {
    rendered_views: &'a mut HashMap<usize, ElementBox>,
    pub font_cache: &'a FontCache,
    pub text_layout_cache: &'a TextLayoutCache,
    pub app: &'a mut MutableAppContext,
    pub window_id: usize,
    pub notify_count: usize,
    view_stack: Vec<usize>,
    handled: bool,
    invalidated_views: HashSet<usize>,
}

impl<'a> EventContext<'a> {
    fn dispatch_event(&mut self, view_id: usize, event: &Event) -> bool {
        if let Some(mut element) = self.rendered_views.remove(&view_id) {
            let result =
                self.with_current_view(view_id, |this| element.dispatch_event(event, this));
            self.rendered_views.insert(view_id, element);
            result
        } else {
            false
        }
    }

    fn with_current_view<F, T>(&mut self, view_id: usize, f: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        self.view_stack.push(view_id);
        let result = f(self);
        self.view_stack.pop();
        result
    }

    pub fn window_id(&self) -> usize {
        self.window_id
    }

    pub fn view_id(&self) -> Option<usize> {
        self.view_stack.last().copied()
    }

    pub fn is_parent_view_focused(&self) -> bool {
        if let Some(parent_view_id) = self.view_stack.last() {
            self.app.focused_view_id(self.window_id) == Some(*parent_view_id)
        } else {
            false
        }
    }

    pub fn focus_parent_view(&mut self) {
        if let Some(parent_view_id) = self.view_stack.last() {
            self.app.focus(self.window_id, Some(*parent_view_id))
        }
    }

    pub fn dispatch_any_action(&mut self, action: Box<dyn Action>) {
        self.app
            .dispatch_any_action_at(self.window_id, *self.view_stack.last().unwrap(), action)
    }

    pub fn dispatch_action<A: Action>(&mut self, action: A) {
        self.dispatch_any_action(Box::new(action));
    }

    pub fn notify(&mut self) {
        self.notify_count += 1;
        if let Some(view_id) = self.view_stack.last() {
            self.invalidated_views.insert(*view_id);
        }
    }

    pub fn notify_count(&self) -> usize {
        self.notify_count
    }

    pub fn propogate_event(&mut self) {
        self.handled = false;
    }
}

impl<'a> Deref for EventContext<'a> {
    type Target = MutableAppContext;

    fn deref(&self) -> &Self::Target {
        self.app
    }
}

impl<'a> DerefMut for EventContext<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.app
    }
}

pub struct MeasurementContext<'a> {
    app: &'a AppContext,
    rendered_views: &'a HashMap<usize, ElementBox>,
    pub window_id: usize,
}

impl<'a> Deref for MeasurementContext<'a> {
    type Target = AppContext;

    fn deref(&self) -> &Self::Target {
        self.app
    }
}

impl<'a> MeasurementContext<'a> {
    fn rect_for_text_range(&self, view_id: usize, range_utf16: Range<usize>) -> Option<RectF> {
        let element = self.rendered_views.get(&view_id)?;
        element.rect_for_text_range(range_utf16, self)
    }
}

pub struct DebugContext<'a> {
    rendered_views: &'a HashMap<usize, ElementBox>,
    pub font_cache: &'a FontCache,
    pub app: &'a AppContext,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    pub fn invert(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }
}

impl ToJson for Axis {
    fn to_json(&self) -> serde_json::Value {
        match self {
            Axis::Horizontal => json!("horizontal"),
            Axis::Vertical => json!("vertical"),
        }
    }
}

pub trait Vector2FExt {
    fn along(self, axis: Axis) -> f32;
}

impl Vector2FExt for Vector2F {
    fn along(self, axis: Axis) -> f32 {
        match axis {
            Axis::Horizontal => self.x(),
            Axis::Vertical => self.y(),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct SizeConstraint {
    pub min: Vector2F,
    pub max: Vector2F,
}

impl SizeConstraint {
    pub fn new(min: Vector2F, max: Vector2F) -> Self {
        Self { min, max }
    }

    pub fn strict(size: Vector2F) -> Self {
        Self {
            min: size,
            max: size,
        }
    }

    pub fn strict_along(axis: Axis, max: f32) -> Self {
        match axis {
            Axis::Horizontal => Self {
                min: vec2f(max, 0.0),
                max: vec2f(max, f32::INFINITY),
            },
            Axis::Vertical => Self {
                min: vec2f(0.0, max),
                max: vec2f(f32::INFINITY, max),
            },
        }
    }

    pub fn max_along(&self, axis: Axis) -> f32 {
        match axis {
            Axis::Horizontal => self.max.x(),
            Axis::Vertical => self.max.y(),
        }
    }

    pub fn min_along(&self, axis: Axis) -> f32 {
        match axis {
            Axis::Horizontal => self.min.x(),
            Axis::Vertical => self.min.y(),
        }
    }

    pub fn constrain(&self, size: Vector2F) -> Vector2F {
        vec2f(
            size.x().min(self.max.x()).max(self.min.x()),
            size.y().min(self.max.y()).max(self.min.y()),
        )
    }
}

impl Default for SizeConstraint {
    fn default() -> Self {
        SizeConstraint {
            min: Vector2F::zero(),
            max: Vector2F::splat(f32::INFINITY),
        }
    }
}

impl ToJson for SizeConstraint {
    fn to_json(&self) -> serde_json::Value {
        json!({
            "min": self.min.to_json(),
            "max": self.max.to_json(),
        })
    }
}

pub struct ChildView {
    view: AnyViewHandle,
}

impl ChildView {
    pub fn new(view: impl Into<AnyViewHandle>) -> Self {
        Self { view: view.into() }
    }
}

impl Element for ChildView {
    type LayoutState = ();
    type PaintState = ();

    fn layout(
        &mut self,
        constraint: SizeConstraint,
        cx: &mut LayoutContext,
    ) -> (Vector2F, Self::LayoutState) {
        let size = cx.layout(self.view.id(), constraint);
        (size, ())
    }

    fn paint(
        &mut self,
        bounds: RectF,
        visible_bounds: RectF,
        _: &mut Self::LayoutState,
        cx: &mut PaintContext,
    ) -> Self::PaintState {
        cx.paint(self.view.id(), bounds.origin(), visible_bounds);
    }

    fn dispatch_event(
        &mut self,
        event: &Event,
        _: RectF,
        _: RectF,
        _: &mut Self::LayoutState,
        _: &mut Self::PaintState,
        cx: &mut EventContext,
    ) -> bool {
        cx.dispatch_event(self.view.id(), event)
    }

    fn rect_for_text_range(
        &self,
        range_utf16: Range<usize>,
        _: RectF,
        _: RectF,
        _: &Self::LayoutState,
        _: &Self::PaintState,
        cx: &MeasurementContext,
    ) -> Option<RectF> {
        cx.rect_for_text_range(self.view.id(), range_utf16)
    }

    fn debug(
        &self,
        bounds: RectF,
        _: &Self::LayoutState,
        _: &Self::PaintState,
        cx: &DebugContext,
    ) -> serde_json::Value {
        json!({
            "type": "ChildView",
            "view_id": self.view.id(),
            "bounds": bounds.to_json(),
            "view": self.view.debug_json(cx.app),
            "child": if let Some(view) = cx.rendered_views.get(&self.view.id()) {
                view.debug(cx)
            } else {
                json!(null)
            }
        })
    }
}
