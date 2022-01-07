use crate::{
    elements::ElementBox,
    executor::{self, Task},
    keymap::{self, Keystroke},
    platform::{self, CursorStyle, Platform, PromptLevel, WindowOptions},
    presenter::Presenter,
    util::{post_inc, timeout},
    AssetCache, AssetSource, ClipboardItem, FontCache, PathPromptOptions, TextLayoutCache,
};
use anyhow::{anyhow, Result};
use keymap::MatchResult;
use parking_lot::Mutex;
use platform::Event;
use postage::{mpsc, sink::Sink as _, stream::Stream as _};
use smol::prelude::*;
use std::{
    any::{type_name, Any, TypeId},
    cell::RefCell,
    collections::{hash_map::Entry, BTreeMap, HashMap, HashSet, VecDeque},
    fmt::{self, Debug},
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    pin::Pin,
    rc::{self, Rc},
    sync::{
        atomic::{AtomicUsize, Ordering::SeqCst},
        Arc, Weak,
    },
    time::Duration,
};

pub trait Entity: 'static {
    type Event;

    fn release(&mut self, _: &mut MutableAppContext) {}
    fn app_will_quit(
        &mut self,
        _: &mut MutableAppContext,
    ) -> Option<Pin<Box<dyn 'static + Future<Output = ()>>>> {
        None
    }
}

pub trait View: Entity + Sized {
    fn ui_name() -> &'static str;
    fn render(&mut self, cx: &mut RenderContext<'_, Self>) -> ElementBox;
    fn on_focus(&mut self, _: &mut ViewContext<Self>) {}
    fn on_blur(&mut self, _: &mut ViewContext<Self>) {}
    fn keymap_context(&self, _: &AppContext) -> keymap::Context {
        Self::default_keymap_context()
    }
    fn default_keymap_context() -> keymap::Context {
        let mut cx = keymap::Context::default();
        cx.set.insert(Self::ui_name().into());
        cx
    }
}

pub trait ReadModel {
    fn read_model<T: Entity>(&self, handle: &ModelHandle<T>) -> &T;
}

pub trait ReadModelWith {
    fn read_model_with<E: Entity, T>(
        &self,
        handle: &ModelHandle<E>,
        read: &mut dyn FnMut(&E, &AppContext) -> T,
    ) -> T;
}

pub trait UpdateModel {
    fn update_model<T: Entity, O>(
        &mut self,
        handle: &ModelHandle<T>,
        update: &mut dyn FnMut(&mut T, &mut ModelContext<T>) -> O,
    ) -> O;
}

pub trait UpgradeModelHandle {
    fn upgrade_model_handle<T: Entity>(&self, handle: WeakModelHandle<T>)
        -> Option<ModelHandle<T>>;
}

pub trait ReadView {
    fn read_view<T: View>(&self, handle: &ViewHandle<T>) -> &T;
}

pub trait ReadViewWith {
    fn read_view_with<V, T>(
        &self,
        handle: &ViewHandle<V>,
        read: &mut dyn FnMut(&V, &AppContext) -> T,
    ) -> T
    where
        V: View;
}

pub trait UpdateView {
    fn update_view<T, S>(
        &mut self,
        handle: &ViewHandle<T>,
        update: &mut dyn FnMut(&mut T, &mut ViewContext<T>) -> S,
    ) -> S
    where
        T: View;
}

pub trait Action: 'static + AnyAction {
    type Argument: 'static + Clone;
}

pub trait AnyAction {
    fn id(&self) -> TypeId;
    fn name(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
    fn boxed_clone(&self) -> Box<dyn AnyAction>;
    fn boxed_clone_as_any(&self) -> Box<dyn Any>;
}

#[macro_export]
macro_rules! action {
    ($name:ident, $arg:ty) => {
        #[derive(Clone)]
        pub struct $name(pub $arg);

        impl $crate::Action for $name {
            type Argument = $arg;
        }

        impl $crate::AnyAction for $name {
            fn id(&self) -> std::any::TypeId {
                std::any::TypeId::of::<$name>()
            }

            fn name(&self) -> &'static str {
                stringify!($name)
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }

            fn boxed_clone(&self) -> Box<dyn $crate::AnyAction> {
                Box::new(self.clone())
            }

            fn boxed_clone_as_any(&self) -> Box<dyn std::any::Any> {
                Box::new(self.clone())
            }
        }
    };

    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct $name;

        impl $crate::Action for $name {
            type Argument = ();
        }

        impl $crate::AnyAction for $name {
            fn id(&self) -> std::any::TypeId {
                std::any::TypeId::of::<$name>()
            }

            fn name(&self) -> &'static str {
                stringify!($name)
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }

            fn boxed_clone(&self) -> Box<dyn $crate::AnyAction> {
                Box::new(self.clone())
            }

            fn boxed_clone_as_any(&self) -> Box<dyn std::any::Any> {
                Box::new(self.clone())
            }
        }
    };
}

pub struct Menu<'a> {
    pub name: &'a str,
    pub items: Vec<MenuItem<'a>>,
}

pub enum MenuItem<'a> {
    Action {
        name: &'a str,
        keystroke: Option<&'a str>,
        action: Box<dyn AnyAction>,
    },
    Separator,
}

#[derive(Clone)]
pub struct App(Rc<RefCell<MutableAppContext>>);

#[derive(Clone)]
pub struct AsyncAppContext(Rc<RefCell<MutableAppContext>>);

#[derive(Clone)]
pub struct TestAppContext {
    cx: Rc<RefCell<MutableAppContext>>,
    foreground_platform: Rc<platform::test::ForegroundPlatform>,
}

impl App {
    pub fn new(asset_source: impl AssetSource) -> Result<Self> {
        let platform = platform::current::platform();
        let foreground_platform = platform::current::foreground_platform();
        let foreground = Rc::new(executor::Foreground::platform(platform.dispatcher())?);
        let app = Self(Rc::new(RefCell::new(MutableAppContext::new(
            foreground,
            Arc::new(executor::Background::new()),
            platform.clone(),
            foreground_platform.clone(),
            Arc::new(FontCache::new(platform.fonts())),
            asset_source,
        ))));

        foreground_platform.on_quit(Box::new({
            let cx = app.0.clone();
            move || {
                cx.borrow_mut().quit();
            }
        }));
        foreground_platform.on_menu_command(Box::new({
            let cx = app.0.clone();
            move |action| {
                let mut cx = cx.borrow_mut();
                if let Some(key_window_id) = cx.cx.platform.key_window_id() {
                    if let Some((presenter, _)) =
                        cx.presenters_and_platform_windows.get(&key_window_id)
                    {
                        let presenter = presenter.clone();
                        let path = presenter.borrow().dispatch_path(cx.as_ref());
                        cx.dispatch_action_any(key_window_id, &path, action);
                    } else {
                        cx.dispatch_global_action_any(action);
                    }
                } else {
                    cx.dispatch_global_action_any(action);
                }
            }
        }));

        app.0.borrow_mut().weak_self = Some(Rc::downgrade(&app.0));
        Ok(app)
    }

    pub fn on_become_active<F>(self, mut callback: F) -> Self
    where
        F: 'static + FnMut(&mut MutableAppContext),
    {
        let cx = self.0.clone();
        self.0
            .borrow_mut()
            .foreground_platform
            .on_become_active(Box::new(move || callback(&mut *cx.borrow_mut())));
        self
    }

    pub fn on_resign_active<F>(self, mut callback: F) -> Self
    where
        F: 'static + FnMut(&mut MutableAppContext),
    {
        let cx = self.0.clone();
        self.0
            .borrow_mut()
            .foreground_platform
            .on_resign_active(Box::new(move || callback(&mut *cx.borrow_mut())));
        self
    }

    pub fn on_quit<F>(self, mut callback: F) -> Self
    where
        F: 'static + FnMut(&mut MutableAppContext),
    {
        let cx = self.0.clone();
        self.0
            .borrow_mut()
            .foreground_platform
            .on_quit(Box::new(move || callback(&mut *cx.borrow_mut())));
        self
    }

    pub fn on_event<F>(self, mut callback: F) -> Self
    where
        F: 'static + FnMut(Event, &mut MutableAppContext) -> bool,
    {
        let cx = self.0.clone();
        self.0
            .borrow_mut()
            .foreground_platform
            .on_event(Box::new(move |event| {
                callback(event, &mut *cx.borrow_mut())
            }));
        self
    }

    pub fn on_open_files<F>(self, mut callback: F) -> Self
    where
        F: 'static + FnMut(Vec<PathBuf>, &mut MutableAppContext),
    {
        let cx = self.0.clone();
        self.0
            .borrow_mut()
            .foreground_platform
            .on_open_files(Box::new(move |paths| {
                callback(paths, &mut *cx.borrow_mut())
            }));
        self
    }

    pub fn run<F>(self, on_finish_launching: F)
    where
        F: 'static + FnOnce(&mut MutableAppContext),
    {
        let platform = self.0.borrow().foreground_platform.clone();
        platform.run(Box::new(move || {
            let mut cx = self.0.borrow_mut();
            let cx = &mut *cx;
            crate::views::init(cx);
            on_finish_launching(cx);
        }))
    }

    pub fn platform(&self) -> Arc<dyn Platform> {
        self.0.borrow().platform()
    }

    pub fn font_cache(&self) -> Arc<FontCache> {
        self.0.borrow().cx.font_cache.clone()
    }

    fn update<T, F: FnOnce(&mut MutableAppContext) -> T>(&mut self, callback: F) -> T {
        let mut state = self.0.borrow_mut();
        let result = state.update(callback);
        state.pending_notifications.clear();
        result
    }
}

impl TestAppContext {
    pub fn new(
        foreground_platform: Rc<platform::test::ForegroundPlatform>,
        platform: Arc<dyn Platform>,
        foreground: Rc<executor::Foreground>,
        background: Arc<executor::Background>,
        font_cache: Arc<FontCache>,
        first_entity_id: usize,
    ) -> Self {
        let mut cx = MutableAppContext::new(
            foreground.clone(),
            background,
            platform,
            foreground_platform.clone(),
            font_cache,
            (),
        );
        cx.next_entity_id = first_entity_id;
        let cx = TestAppContext {
            cx: Rc::new(RefCell::new(cx)),
            foreground_platform,
        };
        cx.cx.borrow_mut().weak_self = Some(Rc::downgrade(&cx.cx));
        cx
    }

    pub fn dispatch_action<A: Action>(
        &self,
        window_id: usize,
        responder_chain: Vec<usize>,
        action: A,
    ) {
        self.cx
            .borrow_mut()
            .dispatch_action_any(window_id, &responder_chain, &action);
    }

    pub fn dispatch_global_action<A: Action>(&self, action: A) {
        self.cx.borrow_mut().dispatch_global_action(action);
    }

    pub fn dispatch_keystroke(
        &self,
        window_id: usize,
        responder_chain: Vec<usize>,
        keystroke: &Keystroke,
    ) -> Result<bool> {
        let mut state = self.cx.borrow_mut();
        state.dispatch_keystroke(window_id, responder_chain, keystroke)
    }

    pub fn add_model<T, F>(&mut self, build_model: F) -> ModelHandle<T>
    where
        T: Entity,
        F: FnOnce(&mut ModelContext<T>) -> T,
    {
        self.cx.borrow_mut().add_model(build_model)
    }

    pub fn add_window<T, F>(&mut self, build_root_view: F) -> (usize, ViewHandle<T>)
    where
        T: View,
        F: FnOnce(&mut ViewContext<T>) -> T,
    {
        self.cx
            .borrow_mut()
            .add_window(Default::default(), build_root_view)
    }

    pub fn window_ids(&self) -> Vec<usize> {
        self.cx.borrow().window_ids().collect()
    }

    pub fn root_view<T: View>(&self, window_id: usize) -> Option<ViewHandle<T>> {
        self.cx.borrow().root_view(window_id)
    }

    pub fn add_view<T, F>(&mut self, window_id: usize, build_view: F) -> ViewHandle<T>
    where
        T: View,
        F: FnOnce(&mut ViewContext<T>) -> T,
    {
        self.cx.borrow_mut().add_view(window_id, build_view)
    }

    pub fn add_option_view<T, F>(
        &mut self,
        window_id: usize,
        build_view: F,
    ) -> Option<ViewHandle<T>>
    where
        T: View,
        F: FnOnce(&mut ViewContext<T>) -> Option<T>,
    {
        self.cx.borrow_mut().add_option_view(window_id, build_view)
    }

    pub fn read<T, F: FnOnce(&AppContext) -> T>(&self, callback: F) -> T {
        callback(self.cx.borrow().as_ref())
    }

    pub fn update<T, F: FnOnce(&mut MutableAppContext) -> T>(&mut self, callback: F) -> T {
        let mut state = self.cx.borrow_mut();
        // Don't increment pending flushes in order to effects to be flushed before the callback
        // completes, which is helpful in tests.
        let result = callback(&mut *state);
        // Flush effects after the callback just in case there are any. This can happen in edge
        // cases such as the closure dropping handles.
        state.flush_effects();
        result
    }

    pub fn to_async(&self) -> AsyncAppContext {
        AsyncAppContext(self.cx.clone())
    }

    pub fn font_cache(&self) -> Arc<FontCache> {
        self.cx.borrow().cx.font_cache.clone()
    }

    pub fn platform(&self) -> Arc<dyn platform::Platform> {
        self.cx.borrow().cx.platform.clone()
    }

    pub fn foreground(&self) -> Rc<executor::Foreground> {
        self.cx.borrow().foreground().clone()
    }

    pub fn background(&self) -> Arc<executor::Background> {
        self.cx.borrow().background().clone()
    }

    pub fn simulate_new_path_selection(&self, result: impl FnOnce(PathBuf) -> Option<PathBuf>) {
        self.foreground_platform.simulate_new_path_selection(result);
    }

    pub fn did_prompt_for_new_path(&self) -> bool {
        self.foreground_platform.as_ref().did_prompt_for_new_path()
    }

    pub fn simulate_prompt_answer(&self, window_id: usize, answer: usize) {
        let mut state = self.cx.borrow_mut();
        let (_, window) = state
            .presenters_and_platform_windows
            .get_mut(&window_id)
            .unwrap();
        let test_window = window
            .as_any_mut()
            .downcast_mut::<platform::test::Window>()
            .unwrap();
        let callback = test_window
            .last_prompt
            .take()
            .expect("prompt was not called");
        (callback)(answer);
    }
}

impl AsyncAppContext {
    pub fn spawn<F, Fut, T>(&self, f: F) -> Task<T>
    where
        F: FnOnce(AsyncAppContext) -> Fut,
        Fut: 'static + Future<Output = T>,
        T: 'static,
    {
        self.0.borrow().foreground.spawn(f(self.clone()))
    }

    pub fn read<T, F: FnOnce(&AppContext) -> T>(&mut self, callback: F) -> T {
        callback(self.0.borrow().as_ref())
    }

    pub fn update<T, F: FnOnce(&mut MutableAppContext) -> T>(&mut self, callback: F) -> T {
        self.0.borrow_mut().update(callback)
    }

    pub fn add_model<T, F>(&mut self, build_model: F) -> ModelHandle<T>
    where
        T: Entity,
        F: FnOnce(&mut ModelContext<T>) -> T,
    {
        self.update(|cx| cx.add_model(build_model))
    }

    pub fn platform(&self) -> Arc<dyn Platform> {
        self.0.borrow().platform()
    }

    pub fn foreground(&self) -> Rc<executor::Foreground> {
        self.0.borrow().foreground.clone()
    }

    pub fn background(&self) -> Arc<executor::Background> {
        self.0.borrow().cx.background.clone()
    }
}

impl UpdateModel for AsyncAppContext {
    fn update_model<E: Entity, O>(
        &mut self,
        handle: &ModelHandle<E>,
        update: &mut dyn FnMut(&mut E, &mut ModelContext<E>) -> O,
    ) -> O {
        self.0.borrow_mut().update_model(handle, update)
    }
}

impl UpgradeModelHandle for AsyncAppContext {
    fn upgrade_model_handle<T: Entity>(
        &self,
        handle: WeakModelHandle<T>,
    ) -> Option<ModelHandle<T>> {
        self.0.borrow_mut().upgrade_model_handle(handle)
    }
}

impl ReadModelWith for AsyncAppContext {
    fn read_model_with<E: Entity, T>(
        &self,
        handle: &ModelHandle<E>,
        read: &mut dyn FnMut(&E, &AppContext) -> T,
    ) -> T {
        let cx = self.0.borrow();
        let cx = cx.as_ref();
        read(handle.read(cx), cx)
    }
}

impl UpdateView for AsyncAppContext {
    fn update_view<T, S>(
        &mut self,
        handle: &ViewHandle<T>,
        update: &mut dyn FnMut(&mut T, &mut ViewContext<T>) -> S,
    ) -> S
    where
        T: View,
    {
        self.0.borrow_mut().update_view(handle, update)
    }
}

impl ReadViewWith for AsyncAppContext {
    fn read_view_with<V, T>(
        &self,
        handle: &ViewHandle<V>,
        read: &mut dyn FnMut(&V, &AppContext) -> T,
    ) -> T
    where
        V: View,
    {
        let cx = self.0.borrow();
        let cx = cx.as_ref();
        read(handle.read(cx), cx)
    }
}

impl UpdateModel for TestAppContext {
    fn update_model<T: Entity, O>(
        &mut self,
        handle: &ModelHandle<T>,
        update: &mut dyn FnMut(&mut T, &mut ModelContext<T>) -> O,
    ) -> O {
        self.cx.borrow_mut().update_model(handle, update)
    }
}

impl ReadModelWith for TestAppContext {
    fn read_model_with<E: Entity, T>(
        &self,
        handle: &ModelHandle<E>,
        read: &mut dyn FnMut(&E, &AppContext) -> T,
    ) -> T {
        let cx = self.cx.borrow();
        let cx = cx.as_ref();
        read(handle.read(cx), cx)
    }
}

impl UpdateView for TestAppContext {
    fn update_view<T, S>(
        &mut self,
        handle: &ViewHandle<T>,
        update: &mut dyn FnMut(&mut T, &mut ViewContext<T>) -> S,
    ) -> S
    where
        T: View,
    {
        self.cx.borrow_mut().update_view(handle, update)
    }
}

impl ReadViewWith for TestAppContext {
    fn read_view_with<V, T>(
        &self,
        handle: &ViewHandle<V>,
        read: &mut dyn FnMut(&V, &AppContext) -> T,
    ) -> T
    where
        V: View,
    {
        let cx = self.cx.borrow();
        let cx = cx.as_ref();
        read(handle.read(cx), cx)
    }
}

type ActionCallback =
    dyn FnMut(&mut dyn AnyView, &dyn AnyAction, &mut MutableAppContext, usize, usize) -> bool;
type GlobalActionCallback = dyn FnMut(&dyn AnyAction, &mut MutableAppContext);

type SubscriptionCallback = Box<dyn FnMut(&dyn Any, &mut MutableAppContext) -> bool>;
type ObservationCallback = Box<dyn FnMut(&mut MutableAppContext) -> bool>;

pub struct MutableAppContext {
    weak_self: Option<rc::Weak<RefCell<Self>>>,
    foreground_platform: Rc<dyn platform::ForegroundPlatform>,
    assets: Arc<AssetCache>,
    cx: AppContext,
    actions: HashMap<TypeId, HashMap<TypeId, Vec<Box<ActionCallback>>>>,
    global_actions: HashMap<TypeId, Box<GlobalActionCallback>>,
    keystroke_matcher: keymap::Matcher,
    next_entity_id: usize,
    next_window_id: usize,
    next_subscription_id: usize,
    subscriptions: Arc<Mutex<HashMap<usize, BTreeMap<usize, SubscriptionCallback>>>>,
    observations: Arc<Mutex<HashMap<usize, BTreeMap<usize, ObservationCallback>>>>,
    presenters_and_platform_windows:
        HashMap<usize, (Rc<RefCell<Presenter>>, Box<dyn platform::Window>)>,
    debug_elements_callbacks: HashMap<usize, Box<dyn Fn(&AppContext) -> crate::json::Value>>,
    foreground: Rc<executor::Foreground>,
    pending_effects: VecDeque<Effect>,
    pending_notifications: HashSet<usize>,
    pending_flushes: usize,
    flushing_effects: bool,
    next_cursor_style_handle_id: Arc<AtomicUsize>,
}

impl MutableAppContext {
    fn new(
        foreground: Rc<executor::Foreground>,
        background: Arc<executor::Background>,
        platform: Arc<dyn platform::Platform>,
        foreground_platform: Rc<dyn platform::ForegroundPlatform>,
        font_cache: Arc<FontCache>,
        asset_source: impl AssetSource,
        // entity_drop_tx:
    ) -> Self {
        Self {
            weak_self: None,
            foreground_platform,
            assets: Arc::new(AssetCache::new(asset_source)),
            cx: AppContext {
                models: Default::default(),
                views: Default::default(),
                windows: Default::default(),
                element_states: Default::default(),
                ref_counts: Arc::new(Mutex::new(RefCounts::default())),
                background,
                font_cache,
                platform,
            },
            actions: HashMap::new(),
            global_actions: HashMap::new(),
            keystroke_matcher: keymap::Matcher::default(),
            next_entity_id: 0,
            next_window_id: 0,
            next_subscription_id: 0,
            subscriptions: Default::default(),
            observations: Default::default(),
            presenters_and_platform_windows: HashMap::new(),
            debug_elements_callbacks: HashMap::new(),
            foreground,
            pending_effects: VecDeque::new(),
            pending_notifications: HashSet::new(),
            pending_flushes: 0,
            flushing_effects: false,
            next_cursor_style_handle_id: Default::default(),
        }
    }

    pub fn upgrade(&self) -> App {
        App(self.weak_self.as_ref().unwrap().upgrade().unwrap())
    }

    pub fn quit(&mut self) {
        let mut futures = Vec::new();
        for model_id in self.cx.models.keys().copied().collect::<Vec<_>>() {
            let mut model = self.cx.models.remove(&model_id).unwrap();
            futures.extend(model.app_will_quit(self));
            self.cx.models.insert(model_id, model);
        }

        for view_id in self.cx.views.keys().copied().collect::<Vec<_>>() {
            let mut view = self.cx.views.remove(&view_id).unwrap();
            futures.extend(view.app_will_quit(self));
            self.cx.views.insert(view_id, view);
        }

        self.remove_all_windows();

        let futures = futures::future::join_all(futures);
        if self
            .background
            .block_with_timeout(Duration::from_millis(100), futures)
            .is_err()
        {
            log::error!("timed out waiting on app_will_quit");
        }
    }

    fn remove_all_windows(&mut self) {
        for (window_id, _) in self.cx.windows.drain() {
            self.presenters_and_platform_windows.remove(&window_id);
        }
        self.remove_dropped_entities();
    }

    pub fn platform(&self) -> Arc<dyn platform::Platform> {
        self.cx.platform.clone()
    }

    pub fn font_cache(&self) -> &Arc<FontCache> {
        &self.cx.font_cache
    }

    pub fn foreground(&self) -> &Rc<executor::Foreground> {
        &self.foreground
    }

    pub fn background(&self) -> &Arc<executor::Background> {
        &self.cx.background
    }

    pub fn on_debug_elements<F>(&mut self, window_id: usize, callback: F)
    where
        F: 'static + Fn(&AppContext) -> crate::json::Value,
    {
        self.debug_elements_callbacks
            .insert(window_id, Box::new(callback));
    }

    pub fn debug_elements(&self, window_id: usize) -> Option<crate::json::Value> {
        self.debug_elements_callbacks
            .get(&window_id)
            .map(|debug_elements| debug_elements(&self.cx))
    }

    pub fn add_action<A, V, F>(&mut self, mut handler: F)
    where
        A: Action,
        V: View,
        F: 'static + FnMut(&mut V, &A, &mut ViewContext<V>),
    {
        let handler = Box::new(
            move |view: &mut dyn AnyView,
                  action: &dyn AnyAction,
                  cx: &mut MutableAppContext,
                  window_id: usize,
                  view_id: usize| {
                let action = action.as_any().downcast_ref().unwrap();
                let mut cx = ViewContext::new(cx, window_id, view_id);
                handler(
                    view.as_any_mut()
                        .downcast_mut()
                        .expect("downcast is type safe"),
                    action,
                    &mut cx,
                );
                cx.halt_action_dispatch
            },
        );

        self.actions
            .entry(TypeId::of::<V>())
            .or_default()
            .entry(TypeId::of::<A>())
            .or_default()
            .push(handler);
    }

    pub fn add_global_action<A, F>(&mut self, mut handler: F)
    where
        A: Action,
        F: 'static + FnMut(&A, &mut MutableAppContext),
    {
        let handler = Box::new(move |action: &dyn AnyAction, cx: &mut MutableAppContext| {
            let action = action.as_any().downcast_ref().unwrap();
            handler(action, cx);
        });

        if self
            .global_actions
            .insert(TypeId::of::<A>(), handler)
            .is_some()
        {
            panic!("registered multiple global handlers for the same action type");
        }
    }

    pub fn window_ids(&self) -> impl Iterator<Item = usize> + '_ {
        self.cx.windows.keys().cloned()
    }

    pub fn root_view<T: View>(&self, window_id: usize) -> Option<ViewHandle<T>> {
        self.cx
            .windows
            .get(&window_id)
            .and_then(|window| window.root_view.clone().downcast::<T>())
    }

    pub fn root_view_id(&self, window_id: usize) -> Option<usize> {
        self.cx.root_view_id(window_id)
    }

    pub fn focused_view_id(&self, window_id: usize) -> Option<usize> {
        self.cx.focused_view_id(window_id)
    }

    pub fn render_view(
        &mut self,
        window_id: usize,
        view_id: usize,
        titlebar_height: f32,
        refreshing: bool,
    ) -> Result<ElementBox> {
        let mut view = self
            .cx
            .views
            .remove(&(window_id, view_id))
            .ok_or(anyhow!("view not found"))?;
        let element = view.render(window_id, view_id, titlebar_height, refreshing, self);
        self.cx.views.insert((window_id, view_id), view);
        Ok(element)
    }

    pub fn render_views(
        &mut self,
        window_id: usize,
        titlebar_height: f32,
    ) -> HashMap<usize, ElementBox> {
        let view_ids = self
            .views
            .keys()
            .filter_map(|(win_id, view_id)| {
                if *win_id == window_id {
                    Some(*view_id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        view_ids
            .into_iter()
            .map(|view_id| {
                (
                    view_id,
                    self.render_view(window_id, view_id, titlebar_height, false)
                        .unwrap(),
                )
            })
            .collect()
    }

    pub fn update<T, F: FnOnce(&mut Self) -> T>(&mut self, callback: F) -> T {
        self.pending_flushes += 1;
        let result = callback(self);
        self.flush_effects();
        result
    }

    pub fn set_menus(&mut self, menus: Vec<Menu>) {
        self.foreground_platform.set_menus(menus);
    }

    fn prompt<F>(
        &self,
        window_id: usize,
        level: PromptLevel,
        msg: &str,
        answers: &[&str],
        done_fn: F,
    ) where
        F: 'static + FnOnce(usize, &mut MutableAppContext),
    {
        let app = self.weak_self.as_ref().unwrap().upgrade().unwrap();
        let foreground = self.foreground.clone();
        let (_, window) = &self.presenters_and_platform_windows[&window_id];
        window.prompt(
            level,
            msg,
            answers,
            Box::new(move |answer| {
                foreground
                    .spawn(async move { (done_fn)(answer, &mut *app.borrow_mut()) })
                    .detach();
            }),
        );
    }

    pub fn prompt_for_paths<F>(&self, options: PathPromptOptions, done_fn: F)
    where
        F: 'static + FnOnce(Option<Vec<PathBuf>>, &mut MutableAppContext),
    {
        let app = self.weak_self.as_ref().unwrap().upgrade().unwrap();
        let foreground = self.foreground.clone();
        self.foreground_platform.prompt_for_paths(
            options,
            Box::new(move |paths| {
                foreground
                    .spawn(async move { (done_fn)(paths, &mut *app.borrow_mut()) })
                    .detach();
            }),
        );
    }

    pub fn prompt_for_new_path<F>(&self, directory: &Path, done_fn: F)
    where
        F: 'static + FnOnce(Option<PathBuf>, &mut MutableAppContext),
    {
        let app = self.weak_self.as_ref().unwrap().upgrade().unwrap();
        let foreground = self.foreground.clone();
        self.foreground_platform.prompt_for_new_path(
            directory,
            Box::new(move |path| {
                foreground
                    .spawn(async move { (done_fn)(path, &mut *app.borrow_mut()) })
                    .detach();
            }),
        );
    }

    pub fn subscribe<E, H, F>(&mut self, handle: &H, mut callback: F) -> Subscription
    where
        E: Entity,
        E::Event: 'static,
        H: Handle<E>,
        F: 'static + FnMut(H, &E::Event, &mut Self),
    {
        self.subscribe_internal(handle, move |handle, event, cx| {
            callback(handle, event, cx);
            true
        })
    }

    pub fn observe<E, H, F>(&mut self, handle: &H, mut callback: F) -> Subscription
    where
        E: Entity,
        E::Event: 'static,
        H: Handle<E>,
        F: 'static + FnMut(H, &mut Self),
    {
        self.observe_internal(handle, move |handle, cx| {
            callback(handle, cx);
            true
        })
    }

    pub fn subscribe_internal<E, H, F>(&mut self, handle: &H, mut callback: F) -> Subscription
    where
        E: Entity,
        E::Event: 'static,
        H: Handle<E>,
        F: 'static + FnMut(H, &E::Event, &mut Self) -> bool,
    {
        let id = post_inc(&mut self.next_subscription_id);
        let emitter = handle.downgrade();
        self.subscriptions
            .lock()
            .entry(handle.id())
            .or_default()
            .insert(
                id,
                Box::new(move |payload, cx| {
                    if let Some(emitter) = H::upgrade_from(&emitter, cx.as_ref()) {
                        let payload = payload.downcast_ref().expect("downcast is type safe");
                        callback(emitter, payload, cx)
                    } else {
                        false
                    }
                }),
            );
        Subscription::Subscription {
            id,
            entity_id: handle.id(),
            subscriptions: Some(Arc::downgrade(&self.subscriptions)),
        }
    }

    fn observe_internal<E, H, F>(&mut self, handle: &H, mut callback: F) -> Subscription
    where
        E: Entity,
        E::Event: 'static,
        H: Handle<E>,
        F: 'static + FnMut(H, &mut Self) -> bool,
    {
        let id = post_inc(&mut self.next_subscription_id);
        let observed = handle.downgrade();
        self.observations
            .lock()
            .entry(handle.id())
            .or_default()
            .insert(
                id,
                Box::new(move |cx| {
                    if let Some(observed) = H::upgrade_from(&observed, cx) {
                        callback(observed, cx)
                    } else {
                        false
                    }
                }),
            );
        Subscription::Observation {
            id,
            entity_id: handle.id(),
            observations: Some(Arc::downgrade(&self.observations)),
        }
    }
    pub(crate) fn notify_model(&mut self, model_id: usize) {
        if self.pending_notifications.insert(model_id) {
            self.pending_effects
                .push_back(Effect::ModelNotification { model_id });
        }
    }

    pub(crate) fn notify_view(&mut self, window_id: usize, view_id: usize) {
        if self.pending_notifications.insert(view_id) {
            self.pending_effects
                .push_back(Effect::ViewNotification { window_id, view_id });
        }
    }

    pub fn dispatch_action<A: Action>(
        &mut self,
        window_id: usize,
        responder_chain: Vec<usize>,
        action: &A,
    ) {
        self.dispatch_action_any(window_id, &responder_chain, action);
    }

    pub(crate) fn dispatch_action_any(
        &mut self,
        window_id: usize,
        path: &[usize],
        action: &dyn AnyAction,
    ) -> bool {
        self.update(|this| {
            let mut halted_dispatch = false;
            for view_id in path.iter().rev() {
                if let Some(mut view) = this.cx.views.remove(&(window_id, *view_id)) {
                    let type_id = view.as_any().type_id();

                    if let Some((name, mut handlers)) = this
                        .actions
                        .get_mut(&type_id)
                        .and_then(|h| h.remove_entry(&action.id()))
                    {
                        for handler in handlers.iter_mut().rev() {
                            let halt_dispatch =
                                handler(view.as_mut(), action, this, window_id, *view_id);
                            if halt_dispatch {
                                halted_dispatch = true;
                                break;
                            }
                        }
                        this.actions
                            .get_mut(&type_id)
                            .unwrap()
                            .insert(name, handlers);
                    }

                    this.cx.views.insert((window_id, *view_id), view);

                    if halted_dispatch {
                        break;
                    }
                }
            }

            if !halted_dispatch {
                halted_dispatch = this.dispatch_global_action_any(action);
            }
            halted_dispatch
        })
    }

    pub fn dispatch_global_action<A: Action>(&mut self, action: A) {
        self.dispatch_global_action_any(&action);
    }

    fn dispatch_global_action_any(&mut self, action: &dyn AnyAction) -> bool {
        self.update(|this| {
            if let Some((name, mut handler)) = this.global_actions.remove_entry(&action.id()) {
                handler(action, this);
                this.global_actions.insert(name, handler);
                true
            } else {
                false
            }
        })
    }

    pub fn add_bindings<T: IntoIterator<Item = keymap::Binding>>(&mut self, bindings: T) {
        self.keystroke_matcher.add_bindings(bindings);
    }

    pub fn dispatch_keystroke(
        &mut self,
        window_id: usize,
        responder_chain: Vec<usize>,
        keystroke: &Keystroke,
    ) -> Result<bool> {
        let mut context_chain = Vec::new();
        for view_id in &responder_chain {
            if let Some(view) = self.cx.views.get(&(window_id, *view_id)) {
                context_chain.push(view.keymap_context(self.as_ref()));
            } else {
                return Err(anyhow!(
                    "View {} in responder chain does not exist",
                    view_id
                ));
            }
        }

        let mut pending = false;
        for (i, cx) in context_chain.iter().enumerate().rev() {
            match self
                .keystroke_matcher
                .push_keystroke(keystroke.clone(), responder_chain[i], cx)
            {
                MatchResult::None => {}
                MatchResult::Pending => pending = true,
                MatchResult::Action(action) => {
                    if self.dispatch_action_any(window_id, &responder_chain[0..=i], action.as_ref())
                    {
                        self.keystroke_matcher.clear_pending();
                        return Ok(true);
                    }
                }
            }
        }

        Ok(pending)
    }

    pub fn add_model<T, F>(&mut self, build_model: F) -> ModelHandle<T>
    where
        T: Entity,
        F: FnOnce(&mut ModelContext<T>) -> T,
    {
        self.update(|this| {
            let model_id = post_inc(&mut this.next_entity_id);
            let handle = ModelHandle::new(model_id, &this.cx.ref_counts);
            let mut cx = ModelContext::new(this, model_id);
            let model = build_model(&mut cx);
            this.cx.models.insert(model_id, Box::new(model));
            handle
        })
    }

    pub fn add_window<T, F>(
        &mut self,
        window_options: WindowOptions,
        build_root_view: F,
    ) -> (usize, ViewHandle<T>)
    where
        T: View,
        F: FnOnce(&mut ViewContext<T>) -> T,
    {
        self.update(|this| {
            let window_id = post_inc(&mut this.next_window_id);
            let root_view = this.add_view(window_id, build_root_view);

            this.cx.windows.insert(
                window_id,
                Window {
                    root_view: root_view.clone().into(),
                    focused_view_id: root_view.id(),
                    invalidation: None,
                },
            );
            this.open_platform_window(window_id, window_options);
            root_view.update(this, |view, cx| {
                view.on_focus(cx);
                cx.notify();
            });

            (window_id, root_view)
        })
    }

    pub fn remove_window(&mut self, window_id: usize) {
        self.cx.windows.remove(&window_id);
        self.presenters_and_platform_windows.remove(&window_id);
        self.remove_dropped_entities();
    }

    fn open_platform_window(&mut self, window_id: usize, window_options: WindowOptions) {
        let mut window =
            self.cx
                .platform
                .open_window(window_id, window_options, self.foreground.clone());
        let presenter = Rc::new(RefCell::new(
            self.build_presenter(window_id, window.titlebar_height()),
        ));

        {
            let mut app = self.upgrade();
            let presenter = presenter.clone();
            window.on_event(Box::new(move |event| {
                app.update(|cx| {
                    if let Event::KeyDown { keystroke, .. } = &event {
                        if cx
                            .dispatch_keystroke(
                                window_id,
                                presenter.borrow().dispatch_path(cx.as_ref()),
                                keystroke,
                            )
                            .unwrap()
                        {
                            return;
                        }
                    }

                    presenter.borrow_mut().dispatch_event(event, cx);
                })
            }));
        }

        {
            let mut app = self.upgrade();
            window.on_resize(Box::new(move || {
                app.update(|cx| cx.resize_window(window_id))
            }));
        }

        {
            let mut app = self.upgrade();
            window.on_close(Box::new(move || {
                app.update(|cx| cx.remove_window(window_id));
            }));
        }

        self.presenters_and_platform_windows
            .insert(window_id, (presenter.clone(), window));

        self.on_debug_elements(window_id, move |cx| {
            presenter.borrow().debug_elements(cx).unwrap()
        });
    }

    pub fn build_presenter(&mut self, window_id: usize, titlebar_height: f32) -> Presenter {
        Presenter::new(
            window_id,
            titlebar_height,
            self.cx.font_cache.clone(),
            TextLayoutCache::new(self.cx.platform.fonts()),
            self.assets.clone(),
            self,
        )
    }

    pub fn build_render_context<V: View>(
        &mut self,
        window_id: usize,
        view_id: usize,
        titlebar_height: f32,
        refreshing: bool,
    ) -> RenderContext<V> {
        RenderContext {
            app: self,
            titlebar_height,
            refreshing,
            window_id,
            view_id,
            view_type: PhantomData,
        }
    }

    pub fn add_view<T, F>(&mut self, window_id: usize, build_view: F) -> ViewHandle<T>
    where
        T: View,
        F: FnOnce(&mut ViewContext<T>) -> T,
    {
        self.add_option_view(window_id, |cx| Some(build_view(cx)))
            .unwrap()
    }

    pub fn add_option_view<T, F>(
        &mut self,
        window_id: usize,
        build_view: F,
    ) -> Option<ViewHandle<T>>
    where
        T: View,
        F: FnOnce(&mut ViewContext<T>) -> Option<T>,
    {
        self.update(|this| {
            let view_id = post_inc(&mut this.next_entity_id);
            let mut cx = ViewContext::new(this, window_id, view_id);
            let handle = if let Some(view) = build_view(&mut cx) {
                this.cx.views.insert((window_id, view_id), Box::new(view));
                if let Some(window) = this.cx.windows.get_mut(&window_id) {
                    window
                        .invalidation
                        .get_or_insert_with(Default::default)
                        .updated
                        .insert(view_id);
                }
                Some(ViewHandle::new(window_id, view_id, &this.cx.ref_counts))
            } else {
                None
            };
            handle
        })
    }

    pub fn element_state<Tag: 'static, T: 'static + Default>(
        &mut self,
        id: ElementStateId,
    ) -> ElementStateHandle<T> {
        let key = (TypeId::of::<Tag>(), id);
        self.cx
            .element_states
            .entry(key)
            .or_insert_with(|| Box::new(T::default()));
        ElementStateHandle::new(TypeId::of::<Tag>(), id, &self.cx.ref_counts)
    }

    fn remove_dropped_entities(&mut self) {
        loop {
            let (dropped_models, dropped_views, dropped_element_states) =
                self.cx.ref_counts.lock().take_dropped();
            if dropped_models.is_empty()
                && dropped_views.is_empty()
                && dropped_element_states.is_empty()
            {
                break;
            }

            for model_id in dropped_models {
                self.subscriptions.lock().remove(&model_id);
                self.observations.lock().remove(&model_id);
                let mut model = self.cx.models.remove(&model_id).unwrap();
                model.release(self);
            }

            for (window_id, view_id) in dropped_views {
                self.subscriptions.lock().remove(&view_id);
                self.observations.lock().remove(&view_id);
                let mut view = self.cx.views.remove(&(window_id, view_id)).unwrap();
                view.release(self);
                let change_focus_to = self.cx.windows.get_mut(&window_id).and_then(|window| {
                    window
                        .invalidation
                        .get_or_insert_with(Default::default)
                        .removed
                        .push(view_id);
                    if window.focused_view_id == view_id {
                        Some(window.root_view.id())
                    } else {
                        None
                    }
                });

                if let Some(view_id) = change_focus_to {
                    self.focus(window_id, view_id);
                }
            }

            for key in dropped_element_states {
                self.cx.element_states.remove(&key);
            }
        }
    }

    fn flush_effects(&mut self) {
        self.pending_flushes = self.pending_flushes.saturating_sub(1);

        if !self.flushing_effects && self.pending_flushes == 0 {
            self.flushing_effects = true;

            let mut refreshing = false;
            loop {
                if let Some(effect) = self.pending_effects.pop_front() {
                    match effect {
                        Effect::Event { entity_id, payload } => self.emit_event(entity_id, payload),
                        Effect::ModelNotification { model_id } => {
                            self.notify_model_observers(model_id)
                        }
                        Effect::ViewNotification { window_id, view_id } => {
                            self.notify_view_observers(window_id, view_id)
                        }
                        Effect::Focus { window_id, view_id } => {
                            self.focus(window_id, view_id);
                        }
                        Effect::ResizeWindow { window_id } => {
                            if let Some(window) = self.cx.windows.get_mut(&window_id) {
                                window
                                    .invalidation
                                    .get_or_insert(WindowInvalidation::default());
                            }
                        }
                        Effect::RefreshWindows => {
                            refreshing = true;
                        }
                    }
                    self.pending_notifications.clear();
                    self.remove_dropped_entities();
                } else {
                    self.remove_dropped_entities();
                    if refreshing {
                        self.perform_window_refresh();
                    } else {
                        self.update_windows();
                    }

                    if self.pending_effects.is_empty() {
                        self.flushing_effects = false;
                        self.pending_notifications.clear();
                        break;
                    } else {
                        refreshing = false;
                    }
                }
            }
        }
    }

    fn update_windows(&mut self) {
        let mut invalidations = HashMap::new();
        for (window_id, window) in &mut self.cx.windows {
            if let Some(invalidation) = window.invalidation.take() {
                invalidations.insert(*window_id, invalidation);
            }
        }

        for (window_id, invalidation) in invalidations {
            if let Some((presenter, mut window)) =
                self.presenters_and_platform_windows.remove(&window_id)
            {
                {
                    let mut presenter = presenter.borrow_mut();
                    presenter.invalidate(invalidation, self);
                    let scene =
                        presenter.build_scene(window.size(), window.scale_factor(), false, self);
                    window.present_scene(scene);
                }
                self.presenters_and_platform_windows
                    .insert(window_id, (presenter, window));
            }
        }
    }

    fn resize_window(&mut self, window_id: usize) {
        self.pending_effects
            .push_back(Effect::ResizeWindow { window_id });
    }

    pub fn refresh_windows(&mut self) {
        self.pending_effects.push_back(Effect::RefreshWindows);
    }

    fn perform_window_refresh(&mut self) {
        let mut presenters = mem::take(&mut self.presenters_and_platform_windows);
        for (window_id, (presenter, window)) in &mut presenters {
            let invalidation = self
                .cx
                .windows
                .get_mut(&window_id)
                .unwrap()
                .invalidation
                .take();
            let mut presenter = presenter.borrow_mut();
            presenter.refresh(invalidation, self);
            let scene = presenter.build_scene(window.size(), window.scale_factor(), true, self);
            window.present_scene(scene);
        }
        self.presenters_and_platform_windows = presenters;
    }

    pub fn set_cursor_style(&mut self, style: CursorStyle) -> CursorStyleHandle {
        self.platform.set_cursor_style(style);
        let id = self.next_cursor_style_handle_id.fetch_add(1, SeqCst);
        CursorStyleHandle {
            id,
            next_cursor_style_handle_id: self.next_cursor_style_handle_id.clone(),
            platform: self.platform(),
        }
    }

    fn emit_event(&mut self, entity_id: usize, payload: Box<dyn Any>) {
        let callbacks = self.subscriptions.lock().remove(&entity_id);
        if let Some(callbacks) = callbacks {
            for (id, mut callback) in callbacks {
                let alive = callback(payload.as_ref(), self);
                if alive {
                    self.subscriptions
                        .lock()
                        .entry(entity_id)
                        .or_default()
                        .insert(id, callback);
                }
            }
        }
    }

    fn notify_model_observers(&mut self, observed_id: usize) {
        let callbacks = self.observations.lock().remove(&observed_id);
        if let Some(callbacks) = callbacks {
            if self.cx.models.contains_key(&observed_id) {
                for (id, mut callback) in callbacks {
                    let alive = callback(self);
                    if alive {
                        self.observations
                            .lock()
                            .entry(observed_id)
                            .or_default()
                            .insert(id, callback);
                    }
                }
            }
        }
    }

    fn notify_view_observers(&mut self, observed_window_id: usize, observed_view_id: usize) {
        if let Some(window) = self.cx.windows.get_mut(&observed_window_id) {
            window
                .invalidation
                .get_or_insert_with(Default::default)
                .updated
                .insert(observed_view_id);
        }

        let callbacks = self.observations.lock().remove(&observed_view_id);
        if let Some(callbacks) = callbacks {
            if self
                .cx
                .views
                .contains_key(&(observed_window_id, observed_view_id))
            {
                for (id, mut callback) in callbacks {
                    let alive = callback(self);
                    if alive {
                        self.observations
                            .lock()
                            .entry(observed_view_id)
                            .or_default()
                            .insert(id, callback);
                    }
                }
            }
        }
    }

    fn focus(&mut self, window_id: usize, focused_id: usize) {
        if self
            .cx
            .windows
            .get(&window_id)
            .map(|w| w.focused_view_id)
            .map_or(false, |cur_focused| cur_focused == focused_id)
        {
            return;
        }

        self.update(|this| {
            let blurred_id = this.cx.windows.get_mut(&window_id).map(|window| {
                let blurred_id = window.focused_view_id;
                window.focused_view_id = focused_id;
                blurred_id
            });

            if let Some(blurred_id) = blurred_id {
                if let Some(mut blurred_view) = this.cx.views.remove(&(window_id, blurred_id)) {
                    blurred_view.on_blur(this, window_id, blurred_id);
                    this.cx.views.insert((window_id, blurred_id), blurred_view);
                }
            }

            if let Some(mut focused_view) = this.cx.views.remove(&(window_id, focused_id)) {
                focused_view.on_focus(this, window_id, focused_id);
                this.cx.views.insert((window_id, focused_id), focused_view);
            }
        })
    }

    pub fn spawn<F, Fut, T>(&self, f: F) -> Task<T>
    where
        F: FnOnce(AsyncAppContext) -> Fut,
        Fut: 'static + Future<Output = T>,
        T: 'static,
    {
        let cx = self.to_async();
        self.foreground.spawn(f(cx))
    }

    pub fn to_async(&self) -> AsyncAppContext {
        AsyncAppContext(self.weak_self.as_ref().unwrap().upgrade().unwrap())
    }

    pub fn write_to_clipboard(&self, item: ClipboardItem) {
        self.cx.platform.write_to_clipboard(item);
    }

    pub fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.cx.platform.read_from_clipboard()
    }
}

impl ReadModel for MutableAppContext {
    fn read_model<T: Entity>(&self, handle: &ModelHandle<T>) -> &T {
        if let Some(model) = self.cx.models.get(&handle.model_id) {
            model
                .as_any()
                .downcast_ref()
                .expect("downcast is type safe")
        } else {
            panic!("circular model reference");
        }
    }
}

impl UpdateModel for MutableAppContext {
    fn update_model<T: Entity, V>(
        &mut self,
        handle: &ModelHandle<T>,
        update: &mut dyn FnMut(&mut T, &mut ModelContext<T>) -> V,
    ) -> V {
        if let Some(mut model) = self.cx.models.remove(&handle.model_id) {
            self.update(|this| {
                let mut cx = ModelContext::new(this, handle.model_id);
                let result = update(
                    model
                        .as_any_mut()
                        .downcast_mut()
                        .expect("downcast is type safe"),
                    &mut cx,
                );
                this.cx.models.insert(handle.model_id, model);
                result
            })
        } else {
            panic!("circular model update");
        }
    }
}

impl UpgradeModelHandle for MutableAppContext {
    fn upgrade_model_handle<T: Entity>(
        &self,
        handle: WeakModelHandle<T>,
    ) -> Option<ModelHandle<T>> {
        self.cx.upgrade_model_handle(handle)
    }
}

impl ReadView for MutableAppContext {
    fn read_view<T: View>(&self, handle: &ViewHandle<T>) -> &T {
        if let Some(view) = self.cx.views.get(&(handle.window_id, handle.view_id)) {
            view.as_any().downcast_ref().expect("downcast is type safe")
        } else {
            panic!("circular view reference");
        }
    }
}

impl UpdateView for MutableAppContext {
    fn update_view<T, S>(
        &mut self,
        handle: &ViewHandle<T>,
        update: &mut dyn FnMut(&mut T, &mut ViewContext<T>) -> S,
    ) -> S
    where
        T: View,
    {
        self.update(|this| {
            let mut view = this
                .cx
                .views
                .remove(&(handle.window_id, handle.view_id))
                .expect("circular view update");

            let mut cx = ViewContext::new(this, handle.window_id, handle.view_id);
            let result = update(
                view.as_any_mut()
                    .downcast_mut()
                    .expect("downcast is type safe"),
                &mut cx,
            );
            this.cx
                .views
                .insert((handle.window_id, handle.view_id), view);
            result
        })
    }
}

impl AsRef<AppContext> for MutableAppContext {
    fn as_ref(&self) -> &AppContext {
        &self.cx
    }
}

impl Deref for MutableAppContext {
    type Target = AppContext;

    fn deref(&self) -> &Self::Target {
        &self.cx
    }
}

pub struct AppContext {
    models: HashMap<usize, Box<dyn AnyModel>>,
    views: HashMap<(usize, usize), Box<dyn AnyView>>,
    windows: HashMap<usize, Window>,
    element_states: HashMap<(TypeId, ElementStateId), Box<dyn Any>>,
    background: Arc<executor::Background>,
    ref_counts: Arc<Mutex<RefCounts>>,
    font_cache: Arc<FontCache>,
    platform: Arc<dyn Platform>,
}

impl AppContext {
    pub fn root_view_id(&self, window_id: usize) -> Option<usize> {
        self.windows
            .get(&window_id)
            .map(|window| window.root_view.id())
    }

    pub fn focused_view_id(&self, window_id: usize) -> Option<usize> {
        self.windows
            .get(&window_id)
            .map(|window| window.focused_view_id)
    }

    pub fn background(&self) -> &Arc<executor::Background> {
        &self.background
    }

    pub fn font_cache(&self) -> &Arc<FontCache> {
        &self.font_cache
    }

    pub fn platform(&self) -> &Arc<dyn Platform> {
        &self.platform
    }
}

impl ReadModel for AppContext {
    fn read_model<T: Entity>(&self, handle: &ModelHandle<T>) -> &T {
        if let Some(model) = self.models.get(&handle.model_id) {
            model
                .as_any()
                .downcast_ref()
                .expect("downcast should be type safe")
        } else {
            panic!("circular model reference");
        }
    }
}

impl UpgradeModelHandle for AppContext {
    fn upgrade_model_handle<T: Entity>(
        &self,
        handle: WeakModelHandle<T>,
    ) -> Option<ModelHandle<T>> {
        if self.models.contains_key(&handle.model_id) {
            Some(ModelHandle::new(handle.model_id, &self.ref_counts))
        } else {
            None
        }
    }
}

impl ReadView for AppContext {
    fn read_view<T: View>(&self, handle: &ViewHandle<T>) -> &T {
        if let Some(view) = self.views.get(&(handle.window_id, handle.view_id)) {
            view.as_any()
                .downcast_ref()
                .expect("downcast should be type safe")
        } else {
            panic!("circular view reference");
        }
    }
}

struct Window {
    root_view: AnyViewHandle,
    focused_view_id: usize,
    invalidation: Option<WindowInvalidation>,
}

#[derive(Default, Clone)]
pub struct WindowInvalidation {
    pub updated: HashSet<usize>,
    pub removed: Vec<usize>,
}

pub enum Effect {
    Event {
        entity_id: usize,
        payload: Box<dyn Any>,
    },
    ModelNotification {
        model_id: usize,
    },
    ViewNotification {
        window_id: usize,
        view_id: usize,
    },
    Focus {
        window_id: usize,
        view_id: usize,
    },
    ResizeWindow {
        window_id: usize,
    },
    RefreshWindows,
}

impl Debug for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effect::Event { entity_id, .. } => f
                .debug_struct("Effect::Event")
                .field("entity_id", entity_id)
                .finish(),
            Effect::ModelNotification { model_id } => f
                .debug_struct("Effect::ModelNotification")
                .field("model_id", model_id)
                .finish(),
            Effect::ViewNotification { window_id, view_id } => f
                .debug_struct("Effect::ViewNotification")
                .field("window_id", window_id)
                .field("view_id", view_id)
                .finish(),
            Effect::Focus { window_id, view_id } => f
                .debug_struct("Effect::Focus")
                .field("window_id", window_id)
                .field("view_id", view_id)
                .finish(),
            Effect::ResizeWindow { window_id } => f
                .debug_struct("Effect::RefreshWindow")
                .field("window_id", window_id)
                .finish(),
            Effect::RefreshWindows => f.debug_struct("Effect::FullViewRefresh").finish(),
        }
    }
}

pub trait AnyModel {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn release(&mut self, cx: &mut MutableAppContext);
    fn app_will_quit(
        &mut self,
        cx: &mut MutableAppContext,
    ) -> Option<Pin<Box<dyn 'static + Future<Output = ()>>>>;
}

impl<T> AnyModel for T
where
    T: Entity,
{
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn release(&mut self, cx: &mut MutableAppContext) {
        self.release(cx);
    }

    fn app_will_quit(
        &mut self,
        cx: &mut MutableAppContext,
    ) -> Option<Pin<Box<dyn 'static + Future<Output = ()>>>> {
        self.app_will_quit(cx)
    }
}

pub trait AnyView {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn release(&mut self, cx: &mut MutableAppContext);
    fn app_will_quit(
        &mut self,
        cx: &mut MutableAppContext,
    ) -> Option<Pin<Box<dyn 'static + Future<Output = ()>>>>;
    fn ui_name(&self) -> &'static str;
    fn render<'a>(
        &mut self,
        window_id: usize,
        view_id: usize,
        titlebar_height: f32,
        refreshing: bool,
        cx: &mut MutableAppContext,
    ) -> ElementBox;
    fn on_focus(&mut self, cx: &mut MutableAppContext, window_id: usize, view_id: usize);
    fn on_blur(&mut self, cx: &mut MutableAppContext, window_id: usize, view_id: usize);
    fn keymap_context(&self, cx: &AppContext) -> keymap::Context;
}

impl<T> AnyView for T
where
    T: View,
{
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn release(&mut self, cx: &mut MutableAppContext) {
        self.release(cx);
    }

    fn app_will_quit(
        &mut self,
        cx: &mut MutableAppContext,
    ) -> Option<Pin<Box<dyn 'static + Future<Output = ()>>>> {
        self.app_will_quit(cx)
    }

    fn ui_name(&self) -> &'static str {
        T::ui_name()
    }

    fn render<'a>(
        &mut self,
        window_id: usize,
        view_id: usize,
        titlebar_height: f32,
        refreshing: bool,
        cx: &mut MutableAppContext,
    ) -> ElementBox {
        View::render(
            self,
            &mut RenderContext {
                window_id,
                view_id,
                app: cx,
                view_type: PhantomData::<T>,
                titlebar_height,
                refreshing,
            },
        )
    }

    fn on_focus(&mut self, cx: &mut MutableAppContext, window_id: usize, view_id: usize) {
        let mut cx = ViewContext::new(cx, window_id, view_id);
        View::on_focus(self, &mut cx);
    }

    fn on_blur(&mut self, cx: &mut MutableAppContext, window_id: usize, view_id: usize) {
        let mut cx = ViewContext::new(cx, window_id, view_id);
        View::on_blur(self, &mut cx);
    }

    fn keymap_context(&self, cx: &AppContext) -> keymap::Context {
        View::keymap_context(self, cx)
    }
}

pub struct ModelContext<'a, T: ?Sized> {
    app: &'a mut MutableAppContext,
    model_id: usize,
    model_type: PhantomData<T>,
    halt_stream: bool,
}

impl<'a, T: Entity> ModelContext<'a, T> {
    fn new(app: &'a mut MutableAppContext, model_id: usize) -> Self {
        Self {
            app,
            model_id,
            model_type: PhantomData,
            halt_stream: false,
        }
    }

    pub fn background(&self) -> &Arc<executor::Background> {
        &self.app.cx.background
    }

    pub fn halt_stream(&mut self) {
        self.halt_stream = true;
    }

    pub fn model_id(&self) -> usize {
        self.model_id
    }

    pub fn add_model<S, F>(&mut self, build_model: F) -> ModelHandle<S>
    where
        S: Entity,
        F: FnOnce(&mut ModelContext<S>) -> S,
    {
        self.app.add_model(build_model)
    }

    pub fn emit(&mut self, payload: T::Event) {
        self.app.pending_effects.push_back(Effect::Event {
            entity_id: self.model_id,
            payload: Box::new(payload),
        });
    }

    pub fn notify(&mut self) {
        self.app.notify_model(self.model_id);
    }

    pub fn subscribe<S: Entity, F>(
        &mut self,
        handle: &ModelHandle<S>,
        mut callback: F,
    ) -> Subscription
    where
        S::Event: 'static,
        F: 'static + FnMut(&mut T, ModelHandle<S>, &S::Event, &mut ModelContext<T>),
    {
        let subscriber = self.weak_handle();
        self.app
            .subscribe_internal(handle, move |emitter, event, cx| {
                if let Some(subscriber) = subscriber.upgrade(cx) {
                    subscriber.update(cx, |subscriber, cx| {
                        callback(subscriber, emitter, event, cx);
                    });
                    true
                } else {
                    false
                }
            })
    }

    pub fn observe<S, F>(&mut self, handle: &ModelHandle<S>, mut callback: F) -> Subscription
    where
        S: Entity,
        F: 'static + FnMut(&mut T, ModelHandle<S>, &mut ModelContext<T>),
    {
        let observer = self.weak_handle();
        self.app.observe_internal(handle, move |observed, cx| {
            if let Some(observer) = observer.upgrade(cx) {
                observer.update(cx, |observer, cx| {
                    callback(observer, observed, cx);
                });
                true
            } else {
                false
            }
        })
    }

    pub fn handle(&self) -> ModelHandle<T> {
        ModelHandle::new(self.model_id, &self.app.cx.ref_counts)
    }

    pub fn weak_handle(&self) -> WeakModelHandle<T> {
        WeakModelHandle::new(self.model_id)
    }

    pub fn spawn<F, Fut, S>(&self, f: F) -> Task<S>
    where
        F: FnOnce(ModelHandle<T>, AsyncAppContext) -> Fut,
        Fut: 'static + Future<Output = S>,
        S: 'static,
    {
        let handle = self.handle();
        self.app.spawn(|cx| f(handle, cx))
    }

    pub fn spawn_weak<F, Fut, S>(&self, f: F) -> Task<S>
    where
        F: FnOnce(WeakModelHandle<T>, AsyncAppContext) -> Fut,
        Fut: 'static + Future<Output = S>,
        S: 'static,
    {
        let handle = self.weak_handle();
        self.app.spawn(|cx| f(handle, cx))
    }
}

impl<M> AsRef<AppContext> for ModelContext<'_, M> {
    fn as_ref(&self) -> &AppContext {
        &self.app.cx
    }
}

impl<M> AsMut<MutableAppContext> for ModelContext<'_, M> {
    fn as_mut(&mut self) -> &mut MutableAppContext {
        self.app
    }
}

impl<M> ReadModel for ModelContext<'_, M> {
    fn read_model<T: Entity>(&self, handle: &ModelHandle<T>) -> &T {
        self.app.read_model(handle)
    }
}

impl<M> UpdateModel for ModelContext<'_, M> {
    fn update_model<T: Entity, V>(
        &mut self,
        handle: &ModelHandle<T>,
        update: &mut dyn FnMut(&mut T, &mut ModelContext<T>) -> V,
    ) -> V {
        self.app.update_model(handle, update)
    }
}

impl<M> UpgradeModelHandle for ModelContext<'_, M> {
    fn upgrade_model_handle<T: Entity>(
        &self,
        handle: WeakModelHandle<T>,
    ) -> Option<ModelHandle<T>> {
        self.cx.upgrade_model_handle(handle)
    }
}

impl<M> Deref for ModelContext<'_, M> {
    type Target = MutableAppContext;

    fn deref(&self) -> &Self::Target {
        &self.app
    }
}

impl<M> DerefMut for ModelContext<'_, M> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.app
    }
}

pub struct ViewContext<'a, T: ?Sized> {
    app: &'a mut MutableAppContext,
    window_id: usize,
    view_id: usize,
    view_type: PhantomData<T>,
    halt_action_dispatch: bool,
}

impl<'a, T: View> ViewContext<'a, T> {
    fn new(app: &'a mut MutableAppContext, window_id: usize, view_id: usize) -> Self {
        Self {
            app,
            window_id,
            view_id,
            view_type: PhantomData,
            halt_action_dispatch: true,
        }
    }

    pub fn handle(&self) -> ViewHandle<T> {
        ViewHandle::new(self.window_id, self.view_id, &self.app.cx.ref_counts)
    }

    pub fn weak_handle(&self) -> WeakViewHandle<T> {
        WeakViewHandle::new(self.window_id, self.view_id)
    }

    pub fn window_id(&self) -> usize {
        self.window_id
    }

    pub fn view_id(&self) -> usize {
        self.view_id
    }

    pub fn foreground(&self) -> &Rc<executor::Foreground> {
        self.app.foreground()
    }

    pub fn background_executor(&self) -> &Arc<executor::Background> {
        &self.app.cx.background
    }

    pub fn platform(&self) -> Arc<dyn Platform> {
        self.app.platform()
    }

    pub fn prompt<F>(&self, level: PromptLevel, msg: &str, answers: &[&str], done_fn: F)
    where
        F: 'static + FnOnce(usize, &mut MutableAppContext),
    {
        self.app
            .prompt(self.window_id, level, msg, answers, done_fn)
    }

    pub fn prompt_for_paths<F>(&self, options: PathPromptOptions, done_fn: F)
    where
        F: 'static + FnOnce(Option<Vec<PathBuf>>, &mut MutableAppContext),
    {
        self.app.prompt_for_paths(options, done_fn)
    }

    pub fn prompt_for_new_path<F>(&self, directory: &Path, done_fn: F)
    where
        F: 'static + FnOnce(Option<PathBuf>, &mut MutableAppContext),
    {
        self.app.prompt_for_new_path(directory, done_fn)
    }

    pub fn debug_elements(&self) -> crate::json::Value {
        self.app.debug_elements(self.window_id).unwrap()
    }

    pub fn focus<S>(&mut self, handle: S)
    where
        S: Into<AnyViewHandle>,
    {
        let handle = handle.into();
        self.app.pending_effects.push_back(Effect::Focus {
            window_id: handle.window_id,
            view_id: handle.view_id,
        });
    }

    pub fn focus_self(&mut self) {
        self.app.pending_effects.push_back(Effect::Focus {
            window_id: self.window_id,
            view_id: self.view_id,
        });
    }

    pub fn add_model<S, F>(&mut self, build_model: F) -> ModelHandle<S>
    where
        S: Entity,
        F: FnOnce(&mut ModelContext<S>) -> S,
    {
        self.app.add_model(build_model)
    }

    pub fn add_view<S, F>(&mut self, build_view: F) -> ViewHandle<S>
    where
        S: View,
        F: FnOnce(&mut ViewContext<S>) -> S,
    {
        self.app.add_view(self.window_id, build_view)
    }

    pub fn add_option_view<S, F>(&mut self, build_view: F) -> Option<ViewHandle<S>>
    where
        S: View,
        F: FnOnce(&mut ViewContext<S>) -> Option<S>,
    {
        self.app.add_option_view(self.window_id, build_view)
    }

    pub fn subscribe<E, H, F>(&mut self, handle: &H, mut callback: F) -> Subscription
    where
        E: Entity,
        E::Event: 'static,
        H: Handle<E>,
        F: 'static + FnMut(&mut T, H, &E::Event, &mut ViewContext<T>),
    {
        let subscriber = self.weak_handle();
        self.app
            .subscribe_internal(handle, move |emitter, event, cx| {
                if let Some(subscriber) = subscriber.upgrade(cx) {
                    subscriber.update(cx, |subscriber, cx| {
                        callback(subscriber, emitter, event, cx);
                    });
                    true
                } else {
                    false
                }
            })
    }

    pub fn observe<E, F, H>(&mut self, handle: &H, mut callback: F) -> Subscription
    where
        E: Entity,
        H: Handle<E>,
        F: 'static + FnMut(&mut T, H, &mut ViewContext<T>),
    {
        let observer = self.weak_handle();
        self.app.observe_internal(handle, move |observed, cx| {
            if let Some(observer) = observer.upgrade(cx) {
                observer.update(cx, |observer, cx| {
                    callback(observer, observed, cx);
                });
                true
            } else {
                false
            }
        })
    }

    pub fn emit(&mut self, payload: T::Event) {
        self.app.pending_effects.push_back(Effect::Event {
            entity_id: self.view_id,
            payload: Box::new(payload),
        });
    }

    pub fn notify(&mut self) {
        self.app.notify_view(self.window_id, self.view_id);
    }

    pub fn propagate_action(&mut self) {
        self.halt_action_dispatch = false;
    }

    pub fn spawn<F, Fut, S>(&self, f: F) -> Task<S>
    where
        F: FnOnce(ViewHandle<T>, AsyncAppContext) -> Fut,
        Fut: 'static + Future<Output = S>,
        S: 'static,
    {
        let handle = self.handle();
        self.app.spawn(|cx| f(handle, cx))
    }

    pub fn spawn_weak<F, Fut, S>(&self, f: F) -> Task<S>
    where
        F: FnOnce(WeakViewHandle<T>, AsyncAppContext) -> Fut,
        Fut: 'static + Future<Output = S>,
        S: 'static,
    {
        let handle = self.weak_handle();
        self.app.spawn(|cx| f(handle, cx))
    }
}

pub struct RenderContext<'a, T: View> {
    pub app: &'a mut MutableAppContext,
    pub titlebar_height: f32,
    pub refreshing: bool,
    window_id: usize,
    view_id: usize,
    view_type: PhantomData<T>,
}

impl<'a, T: View> RenderContext<'a, T> {
    pub fn handle(&self) -> WeakViewHandle<T> {
        WeakViewHandle::new(self.window_id, self.view_id)
    }

    pub fn view_id(&self) -> usize {
        self.view_id
    }
}

impl AsRef<AppContext> for &AppContext {
    fn as_ref(&self) -> &AppContext {
        self
    }
}

impl<V: View> Deref for RenderContext<'_, V> {
    type Target = MutableAppContext;

    fn deref(&self) -> &Self::Target {
        self.app
    }
}

impl<V: View> DerefMut for RenderContext<'_, V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.app
    }
}

impl<V: View> ReadModel for RenderContext<'_, V> {
    fn read_model<T: Entity>(&self, handle: &ModelHandle<T>) -> &T {
        self.app.read_model(handle)
    }
}

impl<V: View> UpdateModel for RenderContext<'_, V> {
    fn update_model<T: Entity, O>(
        &mut self,
        handle: &ModelHandle<T>,
        update: &mut dyn FnMut(&mut T, &mut ModelContext<T>) -> O,
    ) -> O {
        self.app.update_model(handle, update)
    }
}

impl<V: View> ReadView for RenderContext<'_, V> {
    fn read_view<T: View>(&self, handle: &ViewHandle<T>) -> &T {
        self.app.read_view(handle)
    }
}

impl<M> AsRef<AppContext> for ViewContext<'_, M> {
    fn as_ref(&self) -> &AppContext {
        &self.app.cx
    }
}

impl<M> Deref for ViewContext<'_, M> {
    type Target = MutableAppContext;

    fn deref(&self) -> &Self::Target {
        &self.app
    }
}

impl<M> DerefMut for ViewContext<'_, M> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.app
    }
}

impl<M> AsMut<MutableAppContext> for ViewContext<'_, M> {
    fn as_mut(&mut self) -> &mut MutableAppContext {
        self.app
    }
}

impl<V> ReadModel for ViewContext<'_, V> {
    fn read_model<T: Entity>(&self, handle: &ModelHandle<T>) -> &T {
        self.app.read_model(handle)
    }
}

impl<V> UpgradeModelHandle for ViewContext<'_, V> {
    fn upgrade_model_handle<T: Entity>(
        &self,
        handle: WeakModelHandle<T>,
    ) -> Option<ModelHandle<T>> {
        self.cx.upgrade_model_handle(handle)
    }
}

impl<V: View> UpdateModel for ViewContext<'_, V> {
    fn update_model<T: Entity, O>(
        &mut self,
        handle: &ModelHandle<T>,
        update: &mut dyn FnMut(&mut T, &mut ModelContext<T>) -> O,
    ) -> O {
        self.app.update_model(handle, update)
    }
}

impl<V: View> ReadView for ViewContext<'_, V> {
    fn read_view<T: View>(&self, handle: &ViewHandle<T>) -> &T {
        self.app.read_view(handle)
    }
}

impl<V: View> UpdateView for ViewContext<'_, V> {
    fn update_view<T, S>(
        &mut self,
        handle: &ViewHandle<T>,
        update: &mut dyn FnMut(&mut T, &mut ViewContext<T>) -> S,
    ) -> S
    where
        T: View,
    {
        self.app.update_view(handle, update)
    }
}

pub trait Handle<T> {
    type Weak: 'static;
    fn id(&self) -> usize;
    fn location(&self) -> EntityLocation;
    fn downgrade(&self) -> Self::Weak;
    fn upgrade_from(weak: &Self::Weak, cx: &AppContext) -> Option<Self>
    where
        Self: Sized;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum EntityLocation {
    Model(usize),
    View(usize, usize),
}

pub struct ModelHandle<T> {
    model_id: usize,
    model_type: PhantomData<T>,
    ref_counts: Arc<Mutex<RefCounts>>,
}

impl<T: Entity> ModelHandle<T> {
    fn new(model_id: usize, ref_counts: &Arc<Mutex<RefCounts>>) -> Self {
        ref_counts.lock().inc_model(model_id);
        Self {
            model_id,
            model_type: PhantomData,
            ref_counts: ref_counts.clone(),
        }
    }

    pub fn downgrade(&self) -> WeakModelHandle<T> {
        WeakModelHandle::new(self.model_id)
    }

    pub fn id(&self) -> usize {
        self.model_id
    }

    pub fn read<'a, C: ReadModel>(&self, cx: &'a C) -> &'a T {
        cx.read_model(self)
    }

    pub fn read_with<'a, C, F, S>(&self, cx: &C, read: F) -> S
    where
        C: ReadModelWith,
        F: FnOnce(&T, &AppContext) -> S,
    {
        let mut read = Some(read);
        cx.read_model_with(self, &mut |model, cx| {
            let read = read.take().unwrap();
            read(model, cx)
        })
    }

    pub fn update<C, F, S>(&self, cx: &mut C, update: F) -> S
    where
        C: UpdateModel,
        F: FnOnce(&mut T, &mut ModelContext<T>) -> S,
    {
        let mut update = Some(update);
        cx.update_model(self, &mut |model, cx| {
            let update = update.take().unwrap();
            update(model, cx)
        })
    }

    pub fn next_notification(&self, cx: &TestAppContext) -> impl Future<Output = ()> {
        let (mut tx, mut rx) = mpsc::channel(1);
        let mut cx = cx.cx.borrow_mut();
        let subscription = cx.observe(self, move |_, _| {
            tx.blocking_send(()).ok();
        });

        let duration = if std::env::var("CI").is_ok() {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(1)
        };

        async move {
            let notification = timeout(duration, rx.recv())
                .await
                .expect("next notification timed out");
            drop(subscription);
            notification.expect("model dropped while test was waiting for its next notification")
        }
    }

    pub fn next_event(&self, cx: &TestAppContext) -> impl Future<Output = T::Event>
    where
        T::Event: Clone,
    {
        let (mut tx, mut rx) = mpsc::channel(1);
        let mut cx = cx.cx.borrow_mut();
        let subscription = cx.subscribe(self, move |_, event, _| {
            tx.blocking_send(event.clone()).ok();
        });

        let duration = if std::env::var("CI").is_ok() {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(1)
        };

        async move {
            let event = timeout(duration, rx.recv())
                .await
                .expect("next event timed out");
            drop(subscription);
            event.expect("model dropped while test was waiting for its next event")
        }
    }

    pub fn condition(
        &self,
        cx: &TestAppContext,
        mut predicate: impl FnMut(&T, &AppContext) -> bool,
    ) -> impl Future<Output = ()> {
        let (tx, mut rx) = mpsc::channel(1024);

        let mut cx = cx.cx.borrow_mut();
        let subscriptions = (
            cx.observe(self, {
                let mut tx = tx.clone();
                move |_, _| {
                    tx.blocking_send(()).ok();
                }
            }),
            cx.subscribe(self, {
                let mut tx = tx.clone();
                move |_, _, _| {
                    tx.blocking_send(()).ok();
                }
            }),
        );

        let cx = cx.weak_self.as_ref().unwrap().upgrade().unwrap();
        let handle = self.downgrade();
        let duration = if std::env::var("CI").is_ok() {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(1)
        };

        async move {
            timeout(duration, async move {
                loop {
                    {
                        let cx = cx.borrow();
                        let cx = cx.as_ref();
                        if predicate(
                            handle
                                .upgrade(cx)
                                .expect("model dropped with pending condition")
                                .read(cx),
                            cx,
                        ) {
                            break;
                        }
                    }

                    cx.borrow().foreground().start_waiting();
                    rx.recv()
                        .await
                        .expect("model dropped with pending condition");
                    cx.borrow().foreground().finish_waiting();
                }
            })
            .await
            .expect("condition timed out");
            drop(subscriptions);
        }
    }
}

impl<T> Clone for ModelHandle<T> {
    fn clone(&self) -> Self {
        self.ref_counts.lock().inc_model(self.model_id);
        Self {
            model_id: self.model_id,
            model_type: PhantomData,
            ref_counts: self.ref_counts.clone(),
        }
    }
}

impl<T> PartialEq for ModelHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.model_id == other.model_id
    }
}

impl<T> Eq for ModelHandle<T> {}

impl<T> Hash for ModelHandle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.model_id.hash(state);
    }
}

impl<T> std::borrow::Borrow<usize> for ModelHandle<T> {
    fn borrow(&self) -> &usize {
        &self.model_id
    }
}

impl<T> Debug for ModelHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(&format!("ModelHandle<{}>", type_name::<T>()))
            .field(&self.model_id)
            .finish()
    }
}

unsafe impl<T> Send for ModelHandle<T> {}
unsafe impl<T> Sync for ModelHandle<T> {}

impl<T> Drop for ModelHandle<T> {
    fn drop(&mut self) {
        self.ref_counts.lock().dec_model(self.model_id);
    }
}

impl<T: Entity> Handle<T> for ModelHandle<T> {
    type Weak = WeakModelHandle<T>;

    fn id(&self) -> usize {
        self.model_id
    }

    fn location(&self) -> EntityLocation {
        EntityLocation::Model(self.model_id)
    }

    fn downgrade(&self) -> Self::Weak {
        self.downgrade()
    }

    fn upgrade_from(weak: &Self::Weak, cx: &AppContext) -> Option<Self>
    where
        Self: Sized,
    {
        weak.upgrade(cx)
    }
}

pub struct WeakModelHandle<T> {
    model_id: usize,
    model_type: PhantomData<T>,
}

unsafe impl<T> Send for WeakModelHandle<T> {}
unsafe impl<T> Sync for WeakModelHandle<T> {}

impl<T: Entity> WeakModelHandle<T> {
    fn new(model_id: usize) -> Self {
        Self {
            model_id,
            model_type: PhantomData,
        }
    }

    pub fn id(&self) -> usize {
        self.model_id
    }

    pub fn upgrade(self, cx: &impl UpgradeModelHandle) -> Option<ModelHandle<T>> {
        cx.upgrade_model_handle(self)
    }
}

impl<T> Hash for WeakModelHandle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.model_id.hash(state)
    }
}

impl<T> PartialEq for WeakModelHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.model_id == other.model_id
    }
}

impl<T> Eq for WeakModelHandle<T> {}

impl<T> Clone for WeakModelHandle<T> {
    fn clone(&self) -> Self {
        Self {
            model_id: self.model_id,
            model_type: PhantomData,
        }
    }
}

impl<T> Copy for WeakModelHandle<T> {}

pub struct ViewHandle<T> {
    window_id: usize,
    view_id: usize,
    view_type: PhantomData<T>,
    ref_counts: Arc<Mutex<RefCounts>>,
}

impl<T: View> ViewHandle<T> {
    fn new(window_id: usize, view_id: usize, ref_counts: &Arc<Mutex<RefCounts>>) -> Self {
        ref_counts.lock().inc_view(window_id, view_id);
        Self {
            window_id,
            view_id,
            view_type: PhantomData,
            ref_counts: ref_counts.clone(),
        }
    }

    pub fn downgrade(&self) -> WeakViewHandle<T> {
        WeakViewHandle::new(self.window_id, self.view_id)
    }

    pub fn window_id(&self) -> usize {
        self.window_id
    }

    pub fn id(&self) -> usize {
        self.view_id
    }

    pub fn read<'a, C: ReadView>(&self, cx: &'a C) -> &'a T {
        cx.read_view(self)
    }

    pub fn read_with<C, F, S>(&self, cx: &C, read: F) -> S
    where
        C: ReadViewWith,
        F: FnOnce(&T, &AppContext) -> S,
    {
        let mut read = Some(read);
        cx.read_view_with(self, &mut |view, cx| {
            let read = read.take().unwrap();
            read(view, cx)
        })
    }

    pub fn update<C, F, S>(&self, cx: &mut C, update: F) -> S
    where
        C: UpdateView,
        F: FnOnce(&mut T, &mut ViewContext<T>) -> S,
    {
        let mut update = Some(update);
        cx.update_view(self, &mut |view, cx| {
            let update = update.take().unwrap();
            update(view, cx)
        })
    }

    pub fn is_focused(&self, cx: &AppContext) -> bool {
        cx.focused_view_id(self.window_id)
            .map_or(false, |focused_id| focused_id == self.view_id)
    }

    pub fn condition(
        &self,
        cx: &TestAppContext,
        mut predicate: impl FnMut(&T, &AppContext) -> bool,
    ) -> impl Future<Output = ()> {
        let (tx, mut rx) = mpsc::channel(1024);

        let mut cx = cx.cx.borrow_mut();
        let subscriptions = self.update(&mut *cx, |_, cx| {
            (
                cx.observe(self, {
                    let mut tx = tx.clone();
                    move |_, _, _| {
                        tx.blocking_send(()).ok();
                    }
                }),
                cx.subscribe(self, {
                    let mut tx = tx.clone();
                    move |_, _, _, _| {
                        tx.blocking_send(()).ok();
                    }
                }),
            )
        });

        let cx = cx.weak_self.as_ref().unwrap().upgrade().unwrap();
        let handle = self.downgrade();
        let duration = if std::env::var("CI").is_ok() {
            Duration::from_secs(2)
        } else {
            Duration::from_millis(500)
        };

        async move {
            timeout(duration, async move {
                loop {
                    {
                        let cx = cx.borrow();
                        let cx = cx.as_ref();
                        if predicate(
                            handle
                                .upgrade(cx)
                                .expect("view dropped with pending condition")
                                .read(cx),
                            cx,
                        ) {
                            break;
                        }
                    }

                    cx.borrow().foreground().start_waiting();
                    rx.recv()
                        .await
                        .expect("view dropped with pending condition");
                    cx.borrow().foreground().finish_waiting();
                }
            })
            .await
            .expect("condition timed out");
            drop(subscriptions);
        }
    }
}

impl<T> Clone for ViewHandle<T> {
    fn clone(&self) -> Self {
        self.ref_counts
            .lock()
            .inc_view(self.window_id, self.view_id);
        Self {
            window_id: self.window_id,
            view_id: self.view_id,
            view_type: PhantomData,
            ref_counts: self.ref_counts.clone(),
        }
    }
}

impl<T> PartialEq for ViewHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.window_id == other.window_id && self.view_id == other.view_id
    }
}

impl<T> Eq for ViewHandle<T> {}

impl<T> Debug for ViewHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(&format!("ViewHandle<{}>", type_name::<T>()))
            .field("window_id", &self.window_id)
            .field("view_id", &self.view_id)
            .finish()
    }
}

impl<T> Drop for ViewHandle<T> {
    fn drop(&mut self) {
        self.ref_counts
            .lock()
            .dec_view(self.window_id, self.view_id);
    }
}

impl<T: View> Handle<T> for ViewHandle<T> {
    type Weak = WeakViewHandle<T>;

    fn id(&self) -> usize {
        self.view_id
    }

    fn location(&self) -> EntityLocation {
        EntityLocation::View(self.window_id, self.view_id)
    }

    fn downgrade(&self) -> Self::Weak {
        self.downgrade()
    }

    fn upgrade_from(weak: &Self::Weak, cx: &AppContext) -> Option<Self>
    where
        Self: Sized,
    {
        weak.upgrade(cx)
    }
}

pub struct AnyViewHandle {
    window_id: usize,
    view_id: usize,
    view_type: TypeId,
    ref_counts: Arc<Mutex<RefCounts>>,
}

impl AnyViewHandle {
    pub fn id(&self) -> usize {
        self.view_id
    }

    pub fn is<T: 'static>(&self) -> bool {
        TypeId::of::<T>() == self.view_type
    }

    pub fn is_focused(&self, cx: &AppContext) -> bool {
        cx.focused_view_id(self.window_id)
            .map_or(false, |focused_id| focused_id == self.view_id)
    }

    pub fn downcast<T: View>(self) -> Option<ViewHandle<T>> {
        if self.is::<T>() {
            let result = Some(ViewHandle {
                window_id: self.window_id,
                view_id: self.view_id,
                ref_counts: self.ref_counts.clone(),
                view_type: PhantomData,
            });
            unsafe {
                Arc::decrement_strong_count(&self.ref_counts);
            }
            std::mem::forget(self);
            result
        } else {
            None
        }
    }
}

impl Clone for AnyViewHandle {
    fn clone(&self) -> Self {
        self.ref_counts
            .lock()
            .inc_view(self.window_id, self.view_id);
        Self {
            window_id: self.window_id,
            view_id: self.view_id,
            view_type: self.view_type,
            ref_counts: self.ref_counts.clone(),
        }
    }
}

impl From<&AnyViewHandle> for AnyViewHandle {
    fn from(handle: &AnyViewHandle) -> Self {
        handle.clone()
    }
}

impl<T: View> From<&ViewHandle<T>> for AnyViewHandle {
    fn from(handle: &ViewHandle<T>) -> Self {
        handle
            .ref_counts
            .lock()
            .inc_view(handle.window_id, handle.view_id);
        AnyViewHandle {
            window_id: handle.window_id,
            view_id: handle.view_id,
            view_type: TypeId::of::<T>(),
            ref_counts: handle.ref_counts.clone(),
        }
    }
}

impl<T: View> From<ViewHandle<T>> for AnyViewHandle {
    fn from(handle: ViewHandle<T>) -> Self {
        let any_handle = AnyViewHandle {
            window_id: handle.window_id,
            view_id: handle.view_id,
            view_type: TypeId::of::<T>(),
            ref_counts: handle.ref_counts.clone(),
        };
        unsafe {
            Arc::decrement_strong_count(&handle.ref_counts);
        }
        std::mem::forget(handle);
        any_handle
    }
}

impl Drop for AnyViewHandle {
    fn drop(&mut self) {
        self.ref_counts
            .lock()
            .dec_view(self.window_id, self.view_id);
    }
}

pub struct AnyModelHandle {
    model_id: usize,
    ref_counts: Arc<Mutex<RefCounts>>,
}

impl<T: Entity> From<ModelHandle<T>> for AnyModelHandle {
    fn from(handle: ModelHandle<T>) -> Self {
        handle.ref_counts.lock().inc_model(handle.model_id);
        Self {
            model_id: handle.model_id,
            ref_counts: handle.ref_counts.clone(),
        }
    }
}

impl Drop for AnyModelHandle {
    fn drop(&mut self) {
        self.ref_counts.lock().dec_model(self.model_id);
    }
}
pub struct WeakViewHandle<T> {
    window_id: usize,
    view_id: usize,
    view_type: PhantomData<T>,
}

impl<T: View> WeakViewHandle<T> {
    fn new(window_id: usize, view_id: usize) -> Self {
        Self {
            window_id,
            view_id,
            view_type: PhantomData,
        }
    }

    pub fn id(&self) -> usize {
        self.view_id
    }

    pub fn upgrade(&self, cx: &AppContext) -> Option<ViewHandle<T>> {
        if cx.ref_counts.lock().is_entity_alive(self.view_id) {
            Some(ViewHandle::new(
                self.window_id,
                self.view_id,
                &cx.ref_counts,
            ))
        } else {
            None
        }
    }
}

impl<T> Clone for WeakViewHandle<T> {
    fn clone(&self) -> Self {
        Self {
            window_id: self.window_id,
            view_id: self.view_id,
            view_type: PhantomData,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementStateId(usize, usize);

impl From<usize> for ElementStateId {
    fn from(id: usize) -> Self {
        Self(id, 0)
    }
}

impl From<(usize, usize)> for ElementStateId {
    fn from(id: (usize, usize)) -> Self {
        Self(id.0, id.1)
    }
}

pub struct ElementStateHandle<T> {
    value_type: PhantomData<T>,
    tag_type_id: TypeId,
    id: ElementStateId,
    ref_counts: Weak<Mutex<RefCounts>>,
}

impl<T: 'static> ElementStateHandle<T> {
    fn new(tag_type_id: TypeId, id: ElementStateId, ref_counts: &Arc<Mutex<RefCounts>>) -> Self {
        ref_counts.lock().inc_element_state(tag_type_id, id);
        Self {
            value_type: PhantomData,
            tag_type_id,
            id,
            ref_counts: Arc::downgrade(ref_counts),
        }
    }

    pub fn read<'a>(&self, cx: &'a AppContext) -> &'a T {
        cx.element_states
            .get(&(self.tag_type_id, self.id))
            .unwrap()
            .downcast_ref()
            .unwrap()
    }

    pub fn update<C, R>(&self, cx: &mut C, f: impl FnOnce(&mut T, &mut C) -> R) -> R
    where
        C: DerefMut<Target = MutableAppContext>,
    {
        let mut element_state = cx
            .deref_mut()
            .cx
            .element_states
            .remove(&(self.tag_type_id, self.id))
            .unwrap();
        let result = f(element_state.downcast_mut().unwrap(), cx);
        cx.deref_mut()
            .cx
            .element_states
            .insert((self.tag_type_id, self.id), element_state);
        result
    }
}

impl<T> Drop for ElementStateHandle<T> {
    fn drop(&mut self) {
        if let Some(ref_counts) = self.ref_counts.upgrade() {
            ref_counts
                .lock()
                .dec_element_state(self.tag_type_id, self.id);
        }
    }
}

pub struct CursorStyleHandle {
    id: usize,
    next_cursor_style_handle_id: Arc<AtomicUsize>,
    platform: Arc<dyn Platform>,
}

impl Drop for CursorStyleHandle {
    fn drop(&mut self) {
        if self.id + 1 == self.next_cursor_style_handle_id.load(SeqCst) {
            self.platform.set_cursor_style(CursorStyle::Arrow);
        }
    }
}

#[must_use]
pub enum Subscription {
    Subscription {
        id: usize,
        entity_id: usize,
        subscriptions: Option<Weak<Mutex<HashMap<usize, BTreeMap<usize, SubscriptionCallback>>>>>,
    },
    Observation {
        id: usize,
        entity_id: usize,
        observations: Option<Weak<Mutex<HashMap<usize, BTreeMap<usize, ObservationCallback>>>>>,
    },
}

impl Subscription {
    pub fn detach(&mut self) {
        match self {
            Subscription::Subscription { subscriptions, .. } => {
                subscriptions.take();
            }
            Subscription::Observation { observations, .. } => {
                observations.take();
            }
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        match self {
            Subscription::Observation {
                id,
                entity_id,
                observations,
            } => {
                if let Some(observations) = observations.as_ref().and_then(Weak::upgrade) {
                    if let Some(observations) = observations.lock().get_mut(entity_id) {
                        observations.remove(id);
                    }
                }
            }
            Subscription::Subscription {
                id,
                entity_id,
                subscriptions,
            } => {
                if let Some(subscriptions) = subscriptions.as_ref().and_then(Weak::upgrade) {
                    if let Some(subscriptions) = subscriptions.lock().get_mut(entity_id) {
                        subscriptions.remove(id);
                    }
                }
            }
        }
    }
}

#[derive(Default)]
struct RefCounts {
    entity_counts: HashMap<usize, usize>,
    element_state_counts: HashMap<(TypeId, ElementStateId), usize>,
    dropped_models: HashSet<usize>,
    dropped_views: HashSet<(usize, usize)>,
    dropped_element_states: HashSet<(TypeId, ElementStateId)>,
}

impl RefCounts {
    fn inc_model(&mut self, model_id: usize) {
        match self.entity_counts.entry(model_id) {
            Entry::Occupied(mut entry) => {
                *entry.get_mut() += 1;
            }
            Entry::Vacant(entry) => {
                entry.insert(1);
                self.dropped_models.remove(&model_id);
            }
        }
    }

    fn inc_view(&mut self, window_id: usize, view_id: usize) {
        match self.entity_counts.entry(view_id) {
            Entry::Occupied(mut entry) => *entry.get_mut() += 1,
            Entry::Vacant(entry) => {
                entry.insert(1);
                self.dropped_views.remove(&(window_id, view_id));
            }
        }
    }

    fn inc_element_state(&mut self, tag_type_id: TypeId, id: ElementStateId) {
        match self.element_state_counts.entry((tag_type_id, id)) {
            Entry::Occupied(mut entry) => *entry.get_mut() += 1,
            Entry::Vacant(entry) => {
                entry.insert(1);
                self.dropped_element_states.remove(&(tag_type_id, id));
            }
        }
    }

    fn dec_model(&mut self, model_id: usize) {
        let count = self.entity_counts.get_mut(&model_id).unwrap();
        *count -= 1;
        if *count == 0 {
            self.entity_counts.remove(&model_id);
            self.dropped_models.insert(model_id);
        }
    }

    fn dec_view(&mut self, window_id: usize, view_id: usize) {
        let count = self.entity_counts.get_mut(&view_id).unwrap();
        *count -= 1;
        if *count == 0 {
            self.entity_counts.remove(&view_id);
            self.dropped_views.insert((window_id, view_id));
        }
    }

    fn dec_element_state(&mut self, tag_type_id: TypeId, id: ElementStateId) {
        let key = (tag_type_id, id);
        let count = self.element_state_counts.get_mut(&key).unwrap();
        *count -= 1;
        if *count == 0 {
            self.element_state_counts.remove(&key);
            self.dropped_element_states.insert(key);
        }
    }

    fn is_entity_alive(&self, entity_id: usize) -> bool {
        self.entity_counts.contains_key(&entity_id)
    }

    fn take_dropped(
        &mut self,
    ) -> (
        HashSet<usize>,
        HashSet<(usize, usize)>,
        HashSet<(TypeId, ElementStateId)>,
    ) {
        (
            std::mem::take(&mut self.dropped_models),
            std::mem::take(&mut self.dropped_views),
            std::mem::take(&mut self.dropped_element_states),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elements::*;
    use smol::future::poll_once;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    #[crate::test(self)]
    fn test_model_handles(cx: &mut MutableAppContext) {
        struct Model {
            other: Option<ModelHandle<Model>>,
            events: Vec<String>,
        }

        impl Entity for Model {
            type Event = usize;
        }

        impl Model {
            fn new(other: Option<ModelHandle<Self>>, cx: &mut ModelContext<Self>) -> Self {
                if let Some(other) = other.as_ref() {
                    cx.observe(other, |me, _, _| {
                        me.events.push("notified".into());
                    })
                    .detach();
                    cx.subscribe(other, |me, _, event, _| {
                        me.events.push(format!("observed event {}", event));
                    })
                    .detach();
                }

                Self {
                    other,
                    events: Vec::new(),
                }
            }
        }

        let handle_1 = cx.add_model(|cx| Model::new(None, cx));
        let handle_2 = cx.add_model(|cx| Model::new(Some(handle_1.clone()), cx));
        assert_eq!(cx.cx.models.len(), 2);

        handle_1.update(cx, |model, cx| {
            model.events.push("updated".into());
            cx.emit(1);
            cx.notify();
            cx.emit(2);
        });
        assert_eq!(handle_1.read(cx).events, vec!["updated".to_string()]);
        assert_eq!(
            handle_2.read(cx).events,
            vec![
                "observed event 1".to_string(),
                "notified".to_string(),
                "observed event 2".to_string(),
            ]
        );

        handle_2.update(cx, |model, _| {
            drop(handle_1);
            model.other.take();
        });

        assert_eq!(cx.cx.models.len(), 1);
        assert!(cx.subscriptions.lock().is_empty());
        assert!(cx.observations.lock().is_empty());
    }

    #[crate::test(self)]
    fn test_subscribe_and_emit_from_model(cx: &mut MutableAppContext) {
        #[derive(Default)]
        struct Model {
            events: Vec<usize>,
        }

        impl Entity for Model {
            type Event = usize;
        }

        let handle_1 = cx.add_model(|_| Model::default());
        let handle_2 = cx.add_model(|_| Model::default());
        let handle_2b = handle_2.clone();

        handle_1.update(cx, |_, c| {
            c.subscribe(&handle_2, move |model: &mut Model, _, event, c| {
                model.events.push(*event);

                c.subscribe(&handle_2b, |model, _, event, _| {
                    model.events.push(*event * 2);
                })
                .detach();
            })
            .detach();
        });

        handle_2.update(cx, |_, c| c.emit(7));
        assert_eq!(handle_1.read(cx).events, vec![7]);

        handle_2.update(cx, |_, c| c.emit(5));
        assert_eq!(handle_1.read(cx).events, vec![7, 5, 10]);
    }

    #[crate::test(self)]
    fn test_observe_and_notify_from_model(cx: &mut MutableAppContext) {
        #[derive(Default)]
        struct Model {
            count: usize,
            events: Vec<usize>,
        }

        impl Entity for Model {
            type Event = ();
        }

        let handle_1 = cx.add_model(|_| Model::default());
        let handle_2 = cx.add_model(|_| Model::default());
        let handle_2b = handle_2.clone();

        handle_1.update(cx, |_, c| {
            c.observe(&handle_2, move |model, observed, c| {
                model.events.push(observed.read(c).count);
                c.observe(&handle_2b, |model, observed, c| {
                    model.events.push(observed.read(c).count * 2);
                })
                .detach();
            })
            .detach();
        });

        handle_2.update(cx, |model, c| {
            model.count = 7;
            c.notify()
        });
        assert_eq!(handle_1.read(cx).events, vec![7]);

        handle_2.update(cx, |model, c| {
            model.count = 5;
            c.notify()
        });
        assert_eq!(handle_1.read(cx).events, vec![7, 5, 10])
    }

    #[crate::test(self)]
    fn test_view_handles(cx: &mut MutableAppContext) {
        struct View {
            other: Option<ViewHandle<View>>,
            events: Vec<String>,
        }

        impl Entity for View {
            type Event = usize;
        }

        impl super::View for View {
            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }

            fn ui_name() -> &'static str {
                "View"
            }
        }

        impl View {
            fn new(other: Option<ViewHandle<View>>, cx: &mut ViewContext<Self>) -> Self {
                if let Some(other) = other.as_ref() {
                    cx.subscribe(other, |me, _, event, _| {
                        me.events.push(format!("observed event {}", event));
                    })
                    .detach();
                }
                Self {
                    other,
                    events: Vec::new(),
                }
            }
        }

        let (window_id, _) = cx.add_window(Default::default(), |cx| View::new(None, cx));
        let handle_1 = cx.add_view(window_id, |cx| View::new(None, cx));
        let handle_2 = cx.add_view(window_id, |cx| View::new(Some(handle_1.clone()), cx));
        assert_eq!(cx.cx.views.len(), 3);

        handle_1.update(cx, |view, cx| {
            view.events.push("updated".into());
            cx.emit(1);
            cx.emit(2);
        });
        assert_eq!(handle_1.read(cx).events, vec!["updated".to_string()]);
        assert_eq!(
            handle_2.read(cx).events,
            vec![
                "observed event 1".to_string(),
                "observed event 2".to_string(),
            ]
        );

        handle_2.update(cx, |view, _| {
            drop(handle_1);
            view.other.take();
        });

        assert_eq!(cx.cx.views.len(), 2);
        assert!(cx.subscriptions.lock().is_empty());
        assert!(cx.observations.lock().is_empty());
    }

    #[crate::test(self)]
    fn test_add_window(cx: &mut MutableAppContext) {
        struct View {
            mouse_down_count: Arc<AtomicUsize>,
        }

        impl Entity for View {
            type Event = ();
        }

        impl super::View for View {
            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                let mouse_down_count = self.mouse_down_count.clone();
                EventHandler::new(Empty::new().boxed())
                    .on_mouse_down(move |_| {
                        mouse_down_count.fetch_add(1, SeqCst);
                        true
                    })
                    .boxed()
            }

            fn ui_name() -> &'static str {
                "View"
            }
        }

        let mouse_down_count = Arc::new(AtomicUsize::new(0));
        let (window_id, _) = cx.add_window(Default::default(), |_| View {
            mouse_down_count: mouse_down_count.clone(),
        });
        let presenter = cx.presenters_and_platform_windows[&window_id].0.clone();
        // Ensure window's root element is in a valid lifecycle state.
        presenter.borrow_mut().dispatch_event(
            Event::LeftMouseDown {
                position: Default::default(),
                ctrl: false,
                alt: false,
                shift: false,
                cmd: false,
                click_count: 1,
            },
            cx,
        );
        assert_eq!(mouse_down_count.load(SeqCst), 1);
    }

    #[crate::test(self)]
    fn test_entity_release_hooks(cx: &mut MutableAppContext) {
        struct Model {
            released: Arc<Mutex<bool>>,
        }

        struct View {
            released: Arc<Mutex<bool>>,
        }

        impl Entity for Model {
            type Event = ();

            fn release(&mut self, _: &mut MutableAppContext) {
                *self.released.lock() = true;
            }
        }

        impl Entity for View {
            type Event = ();

            fn release(&mut self, _: &mut MutableAppContext) {
                *self.released.lock() = true;
            }
        }

        impl super::View for View {
            fn ui_name() -> &'static str {
                "View"
            }

            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }
        }

        let model_released = Arc::new(Mutex::new(false));
        let view_released = Arc::new(Mutex::new(false));

        let model = cx.add_model(|_| Model {
            released: model_released.clone(),
        });

        let (window_id, _) = cx.add_window(Default::default(), |_| View {
            released: view_released.clone(),
        });

        assert!(!*model_released.lock());
        assert!(!*view_released.lock());

        cx.update(move |_| {
            drop(model);
        });
        assert!(*model_released.lock());

        drop(cx.remove_window(window_id));
        assert!(*view_released.lock());
    }

    #[crate::test(self)]
    fn test_subscribe_and_emit_from_view(cx: &mut MutableAppContext) {
        #[derive(Default)]
        struct View {
            events: Vec<usize>,
        }

        impl Entity for View {
            type Event = usize;
        }

        impl super::View for View {
            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }

            fn ui_name() -> &'static str {
                "View"
            }
        }

        struct Model;

        impl Entity for Model {
            type Event = usize;
        }

        let (window_id, handle_1) = cx.add_window(Default::default(), |_| View::default());
        let handle_2 = cx.add_view(window_id, |_| View::default());
        let handle_2b = handle_2.clone();
        let handle_3 = cx.add_model(|_| Model);

        handle_1.update(cx, |_, c| {
            c.subscribe(&handle_2, move |me, _, event, c| {
                me.events.push(*event);

                c.subscribe(&handle_2b, |me, _, event, _| {
                    me.events.push(*event * 2);
                })
                .detach();
            })
            .detach();

            c.subscribe(&handle_3, |me, _, event, _| {
                me.events.push(*event);
            })
            .detach();
        });

        handle_2.update(cx, |_, c| c.emit(7));
        assert_eq!(handle_1.read(cx).events, vec![7]);

        handle_2.update(cx, |_, c| c.emit(5));
        assert_eq!(handle_1.read(cx).events, vec![7, 5, 10]);

        handle_3.update(cx, |_, c| c.emit(9));
        assert_eq!(handle_1.read(cx).events, vec![7, 5, 10, 9]);
    }

    #[crate::test(self)]
    fn test_dropping_subscribers(cx: &mut MutableAppContext) {
        struct View;

        impl Entity for View {
            type Event = ();
        }

        impl super::View for View {
            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }

            fn ui_name() -> &'static str {
                "View"
            }
        }

        struct Model;

        impl Entity for Model {
            type Event = ();
        }

        let (window_id, _) = cx.add_window(Default::default(), |_| View);
        let observing_view = cx.add_view(window_id, |_| View);
        let emitting_view = cx.add_view(window_id, |_| View);
        let observing_model = cx.add_model(|_| Model);
        let observed_model = cx.add_model(|_| Model);

        observing_view.update(cx, |_, cx| {
            cx.subscribe(&emitting_view, |_, _, _, _| {}).detach();
            cx.subscribe(&observed_model, |_, _, _, _| {}).detach();
        });
        observing_model.update(cx, |_, cx| {
            cx.subscribe(&observed_model, |_, _, _, _| {}).detach();
        });

        cx.update(|_| {
            drop(observing_view);
            drop(observing_model);
        });

        emitting_view.update(cx, |_, cx| cx.emit(()));
        observed_model.update(cx, |_, cx| cx.emit(()));
    }

    #[crate::test(self)]
    fn test_observe_and_notify_from_view(cx: &mut MutableAppContext) {
        #[derive(Default)]
        struct View {
            events: Vec<usize>,
        }

        impl Entity for View {
            type Event = usize;
        }

        impl super::View for View {
            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }

            fn ui_name() -> &'static str {
                "View"
            }
        }

        #[derive(Default)]
        struct Model {
            count: usize,
        }

        impl Entity for Model {
            type Event = ();
        }

        let (_, view) = cx.add_window(Default::default(), |_| View::default());
        let model = cx.add_model(|_| Model::default());

        view.update(cx, |_, c| {
            c.observe(&model, |me, observed, c| {
                me.events.push(observed.read(c).count)
            })
            .detach();
        });

        model.update(cx, |model, c| {
            model.count = 11;
            c.notify();
        });
        assert_eq!(view.read(cx).events, vec![11]);
    }

    #[crate::test(self)]
    fn test_dropping_observers(cx: &mut MutableAppContext) {
        struct View;

        impl Entity for View {
            type Event = ();
        }

        impl super::View for View {
            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }

            fn ui_name() -> &'static str {
                "View"
            }
        }

        struct Model;

        impl Entity for Model {
            type Event = ();
        }

        let (window_id, _) = cx.add_window(Default::default(), |_| View);
        let observing_view = cx.add_view(window_id, |_| View);
        let observing_model = cx.add_model(|_| Model);
        let observed_model = cx.add_model(|_| Model);

        observing_view.update(cx, |_, cx| {
            cx.observe(&observed_model, |_, _, _| {}).detach();
        });
        observing_model.update(cx, |_, cx| {
            cx.observe(&observed_model, |_, _, _| {}).detach();
        });

        cx.update(|_| {
            drop(observing_view);
            drop(observing_model);
        });

        observed_model.update(cx, |_, cx| cx.notify());
    }

    #[crate::test(self)]
    fn test_focus(cx: &mut MutableAppContext) {
        struct View {
            name: String,
            events: Arc<Mutex<Vec<String>>>,
        }

        impl Entity for View {
            type Event = ();
        }

        impl super::View for View {
            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }

            fn ui_name() -> &'static str {
                "View"
            }

            fn on_focus(&mut self, _: &mut ViewContext<Self>) {
                self.events.lock().push(format!("{} focused", &self.name));
            }

            fn on_blur(&mut self, _: &mut ViewContext<Self>) {
                self.events.lock().push(format!("{} blurred", &self.name));
            }
        }

        let events: Arc<Mutex<Vec<String>>> = Default::default();
        let (window_id, view_1) = cx.add_window(Default::default(), |_| View {
            events: events.clone(),
            name: "view 1".to_string(),
        });
        let view_2 = cx.add_view(window_id, |_| View {
            events: events.clone(),
            name: "view 2".to_string(),
        });

        view_1.update(cx, |_, cx| cx.focus(&view_2));
        view_1.update(cx, |_, cx| cx.focus(&view_1));
        view_1.update(cx, |_, cx| cx.focus(&view_2));
        view_1.update(cx, |_, _| drop(view_2));

        assert_eq!(
            *events.lock(),
            [
                "view 1 focused".to_string(),
                "view 1 blurred".to_string(),
                "view 2 focused".to_string(),
                "view 2 blurred".to_string(),
                "view 1 focused".to_string(),
                "view 1 blurred".to_string(),
                "view 2 focused".to_string(),
                "view 1 focused".to_string(),
            ],
        );
    }

    #[crate::test(self)]
    fn test_dispatch_action(cx: &mut MutableAppContext) {
        struct ViewA {
            id: usize,
        }

        impl Entity for ViewA {
            type Event = ();
        }

        impl View for ViewA {
            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }

            fn ui_name() -> &'static str {
                "View"
            }
        }

        struct ViewB {
            id: usize,
        }

        impl Entity for ViewB {
            type Event = ();
        }

        impl View for ViewB {
            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }

            fn ui_name() -> &'static str {
                "View"
            }
        }

        action!(Action, &'static str);

        let actions = Rc::new(RefCell::new(Vec::new()));

        let actions_clone = actions.clone();
        cx.add_global_action(move |_: &Action, _: &mut MutableAppContext| {
            actions_clone.borrow_mut().push("global".to_string());
        });

        let actions_clone = actions.clone();
        cx.add_action(move |view: &mut ViewA, action: &Action, cx| {
            assert_eq!(action.0, "bar");
            cx.propagate_action();
            actions_clone.borrow_mut().push(format!("{} a", view.id));
        });

        let actions_clone = actions.clone();
        cx.add_action(move |view: &mut ViewA, _: &Action, cx| {
            if view.id != 1 {
                cx.propagate_action();
            }
            actions_clone.borrow_mut().push(format!("{} b", view.id));
        });

        let actions_clone = actions.clone();
        cx.add_action(move |view: &mut ViewB, _: &Action, cx| {
            cx.propagate_action();
            actions_clone.borrow_mut().push(format!("{} c", view.id));
        });

        let actions_clone = actions.clone();
        cx.add_action(move |view: &mut ViewB, _: &Action, cx| {
            cx.propagate_action();
            actions_clone.borrow_mut().push(format!("{} d", view.id));
        });

        let (window_id, view_1) = cx.add_window(Default::default(), |_| ViewA { id: 1 });
        let view_2 = cx.add_view(window_id, |_| ViewB { id: 2 });
        let view_3 = cx.add_view(window_id, |_| ViewA { id: 3 });
        let view_4 = cx.add_view(window_id, |_| ViewB { id: 4 });

        cx.dispatch_action(
            window_id,
            vec![view_1.id(), view_2.id(), view_3.id(), view_4.id()],
            &Action("bar"),
        );

        assert_eq!(
            *actions.borrow(),
            vec!["4 d", "4 c", "3 b", "3 a", "2 d", "2 c", "1 b"]
        );

        // Remove view_1, which doesn't propagate the action
        actions.borrow_mut().clear();
        cx.dispatch_action(
            window_id,
            vec![view_2.id(), view_3.id(), view_4.id()],
            &Action("bar"),
        );

        assert_eq!(
            *actions.borrow(),
            vec!["4 d", "4 c", "3 b", "3 a", "2 d", "2 c", "global"]
        );
    }

    #[crate::test(self)]
    fn test_dispatch_keystroke(cx: &mut MutableAppContext) {
        use std::cell::Cell;

        action!(Action, &'static str);

        struct View {
            id: usize,
            keymap_context: keymap::Context,
        }

        impl Entity for View {
            type Event = ();
        }

        impl super::View for View {
            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }

            fn ui_name() -> &'static str {
                "View"
            }

            fn keymap_context(&self, _: &AppContext) -> keymap::Context {
                self.keymap_context.clone()
            }
        }

        impl View {
            fn new(id: usize) -> Self {
                View {
                    id,
                    keymap_context: keymap::Context::default(),
                }
            }
        }

        let mut view_1 = View::new(1);
        let mut view_2 = View::new(2);
        let mut view_3 = View::new(3);
        view_1.keymap_context.set.insert("a".into());
        view_2.keymap_context.set.insert("a".into());
        view_2.keymap_context.set.insert("b".into());
        view_3.keymap_context.set.insert("a".into());
        view_3.keymap_context.set.insert("b".into());
        view_3.keymap_context.set.insert("c".into());

        let (window_id, view_1) = cx.add_window(Default::default(), |_| view_1);
        let view_2 = cx.add_view(window_id, |_| view_2);
        let view_3 = cx.add_view(window_id, |_| view_3);

        // This keymap's only binding dispatches an action on view 2 because that view will have
        // "a" and "b" in its context, but not "c".
        cx.add_bindings(vec![keymap::Binding::new(
            "a",
            Action("a"),
            Some("a && b && !c"),
        )]);

        let handled_action = Rc::new(Cell::new(false));
        let handled_action_clone = handled_action.clone();
        cx.add_action(move |view: &mut View, action: &Action, _| {
            handled_action_clone.set(true);
            assert_eq!(view.id, 2);
            assert_eq!(action.0, "a");
        });

        cx.dispatch_keystroke(
            window_id,
            vec![view_1.id(), view_2.id(), view_3.id()],
            &Keystroke::parse("a").unwrap(),
        )
        .unwrap();

        assert!(handled_action.get());
    }

    #[crate::test(self)]
    async fn test_model_condition(mut cx: TestAppContext) {
        struct Counter(usize);

        impl super::Entity for Counter {
            type Event = ();
        }

        impl Counter {
            fn inc(&mut self, cx: &mut ModelContext<Self>) {
                self.0 += 1;
                cx.notify();
            }
        }

        let model = cx.add_model(|_| Counter(0));

        let condition1 = model.condition(&cx, |model, _| model.0 == 2);
        let condition2 = model.condition(&cx, |model, _| model.0 == 3);
        smol::pin!(condition1, condition2);

        model.update(&mut cx, |model, cx| model.inc(cx));
        assert_eq!(poll_once(&mut condition1).await, None);
        assert_eq!(poll_once(&mut condition2).await, None);

        model.update(&mut cx, |model, cx| model.inc(cx));
        assert_eq!(poll_once(&mut condition1).await, Some(()));
        assert_eq!(poll_once(&mut condition2).await, None);

        model.update(&mut cx, |model, cx| model.inc(cx));
        assert_eq!(poll_once(&mut condition2).await, Some(()));

        model.update(&mut cx, |_, cx| cx.notify());
    }

    #[crate::test(self)]
    #[should_panic]
    async fn test_model_condition_timeout(mut cx: TestAppContext) {
        struct Model;

        impl super::Entity for Model {
            type Event = ();
        }

        let model = cx.add_model(|_| Model);
        model.condition(&cx, |_, _| false).await;
    }

    #[crate::test(self)]
    #[should_panic(expected = "model dropped with pending condition")]
    async fn test_model_condition_panic_on_drop(mut cx: TestAppContext) {
        struct Model;

        impl super::Entity for Model {
            type Event = ();
        }

        let model = cx.add_model(|_| Model);
        let condition = model.condition(&cx, |_, _| false);
        cx.update(|_| drop(model));
        condition.await;
    }

    #[crate::test(self)]
    async fn test_view_condition(mut cx: TestAppContext) {
        struct Counter(usize);

        impl super::Entity for Counter {
            type Event = ();
        }

        impl super::View for Counter {
            fn ui_name() -> &'static str {
                "test view"
            }

            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }
        }

        impl Counter {
            fn inc(&mut self, cx: &mut ViewContext<Self>) {
                self.0 += 1;
                cx.notify();
            }
        }

        let (_, view) = cx.add_window(|_| Counter(0));

        let condition1 = view.condition(&cx, |view, _| view.0 == 2);
        let condition2 = view.condition(&cx, |view, _| view.0 == 3);
        smol::pin!(condition1, condition2);

        view.update(&mut cx, |view, cx| view.inc(cx));
        assert_eq!(poll_once(&mut condition1).await, None);
        assert_eq!(poll_once(&mut condition2).await, None);

        view.update(&mut cx, |view, cx| view.inc(cx));
        assert_eq!(poll_once(&mut condition1).await, Some(()));
        assert_eq!(poll_once(&mut condition2).await, None);

        view.update(&mut cx, |view, cx| view.inc(cx));
        assert_eq!(poll_once(&mut condition2).await, Some(()));
        view.update(&mut cx, |_, cx| cx.notify());
    }

    #[crate::test(self)]
    #[should_panic]
    async fn test_view_condition_timeout(mut cx: TestAppContext) {
        struct View;

        impl super::Entity for View {
            type Event = ();
        }

        impl super::View for View {
            fn ui_name() -> &'static str {
                "test view"
            }

            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }
        }

        let (_, view) = cx.add_window(|_| View);
        view.condition(&cx, |_, _| false).await;
    }

    #[crate::test(self)]
    #[should_panic(expected = "view dropped with pending condition")]
    async fn test_view_condition_panic_on_drop(mut cx: TestAppContext) {
        struct View;

        impl super::Entity for View {
            type Event = ();
        }

        impl super::View for View {
            fn ui_name() -> &'static str {
                "test view"
            }

            fn render(&mut self, _: &mut RenderContext<Self>) -> ElementBox {
                Empty::new().boxed()
            }
        }

        let window_id = cx.add_window(|_| View).0;
        let view = cx.add_view(window_id, |_| View);

        let condition = view.condition(&cx, |_, _| false);
        cx.update(|_| drop(view));
        condition.await;
    }
}
