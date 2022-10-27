use crate::Action;
use anyhow::{anyhow, Result};
use collections::BTreeMap;
use smallvec::SmallVec;
use std::{
    any::{Any, TypeId},
    collections::{HashMap, HashSet},
    fmt::{Debug, Write},
};
use tree_sitter::{Language, Node, Parser};

extern "C" {
    fn tree_sitter_context_predicate() -> Language;
}

pub struct Matcher {
    pending: HashMap<usize, Pending>,
    keymap: Keymap,
}

#[derive(Default)]
struct Pending {
    keystrokes: Vec<Keystroke>,
    context: Option<Context>,
}

#[derive(Default)]
pub struct Keymap {
    bindings: Vec<Binding>,
    binding_indices_by_action_type: HashMap<TypeId, SmallVec<[usize; 3]>>,
}

pub struct Binding {
    keystrokes: SmallVec<[Keystroke; 2]>,
    action: Box<dyn Action>,
    context_predicate: Option<ContextPredicate>,
}

impl Debug for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Binding")
            .field("keystrokes", &self.keystrokes)
            .field(&self.action.name(), &"..")
            .field("context_predicate", &self.context_predicate)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct Keystroke {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
    pub function: bool,
    pub key: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Context {
    pub set: HashSet<String>,
    pub map: HashMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ContextPredicate {
    Identifier(String),
    Equal(String, String),
    NotEqual(String, String),
    Not(Box<ContextPredicate>),
    And(Box<ContextPredicate>, Box<ContextPredicate>),
    Or(Box<ContextPredicate>, Box<ContextPredicate>),
}

trait ActionArg {
    fn boxed_clone(&self) -> Box<dyn Any>;
}

impl<T> ActionArg for T
where
    T: 'static + Any + Clone,
{
    fn boxed_clone(&self) -> Box<dyn Any> {
        Box::new(self.clone())
    }
}

pub enum MatchResult {
    None,
    Pending,
    Match {
        view_id: usize,
        action: Box<dyn Action>,
    },
}

impl Debug for MatchResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchResult::None => f.debug_struct("MatchResult2::None").finish(),
            MatchResult::Pending => f.debug_struct("MatchResult2::Pending").finish(),
            MatchResult::Match { view_id, action } => f
                .debug_struct("MatchResult::Match")
                .field("view_id", view_id)
                .field("action", &action.name())
                .finish(),
        }
    }
}

impl PartialEq for MatchResult {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (MatchResult::None, MatchResult::None) => true,
            (MatchResult::Pending, MatchResult::Pending) => true,
            (
                MatchResult::Match { view_id, action },
                MatchResult::Match {
                    view_id: other_view_id,
                    action: other_action,
                },
            ) => view_id == other_view_id && action.eq(other_action.as_ref()),
            _ => false,
        }
    }
}

impl Clone for MatchResult {
    fn clone(&self) -> Self {
        match self {
            MatchResult::None => MatchResult::None,
            MatchResult::Pending => MatchResult::Pending,
            MatchResult::Match { view_id, action } => MatchResult::Match {
                view_id: *view_id,
                action: Action::boxed_clone(action.as_ref()),
            },
        }
    }
}

impl Eq for MatchResult {}

impl Matcher {
    pub fn new(keymap: Keymap) -> Self {
        Self {
            pending: HashMap::new(),
            keymap,
        }
    }

    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.pending.clear();
        self.keymap = keymap;
    }

    pub fn add_bindings<T: IntoIterator<Item = Binding>>(&mut self, bindings: T) {
        self.pending.clear();
        self.keymap.add_bindings(bindings);
    }

    pub fn clear_bindings(&mut self) {
        self.pending.clear();
        self.keymap.clear();
    }

    pub fn bindings_for_action_type(&self, action_type: TypeId) -> impl Iterator<Item = &Binding> {
        self.keymap.bindings_for_action_type(action_type)
    }

    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    pub fn has_pending_keystrokes(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn push_keystroke(
        &mut self,
        keystroke: Keystroke,
        dispatch_path: Vec<(usize, Context)>,
    ) -> MatchResult {
        let mut any_pending = false;

        let first_keystroke = self.pending.is_empty();
        for (view_id, context) in dispatch_path {
            if !first_keystroke && !self.pending.contains_key(&view_id) {
                continue;
            }

            let pending = self.pending.entry(view_id).or_default();

            if let Some(pending_context) = pending.context.as_ref() {
                if pending_context != &context {
                    pending.keystrokes.clear();
                }
            }

            pending.keystrokes.push(keystroke.clone());

            let mut retain_pending = false;
            for binding in self.keymap.bindings.iter().rev() {
                if binding.keystrokes.starts_with(&pending.keystrokes)
                    && binding
                        .context_predicate
                        .as_ref()
                        .map(|c| c.eval(&context))
                        .unwrap_or(true)
                {
                    if binding.keystrokes.len() == pending.keystrokes.len() {
                        self.pending.remove(&view_id);
                        return MatchResult::Match {
                            view_id,
                            action: binding.action.boxed_clone(),
                        };
                    } else {
                        retain_pending = true;
                        pending.context = Some(context.clone());
                    }
                }
            }

            if retain_pending {
                any_pending = true;
            } else {
                self.pending.remove(&view_id);
            }
        }

        if any_pending {
            MatchResult::Pending
        } else {
            MatchResult::None
        }
    }

    pub fn keystrokes_for_action(
        &self,
        action: &dyn Action,
        cx: &Context,
    ) -> Option<SmallVec<[Keystroke; 2]>> {
        for binding in self.keymap.bindings.iter().rev() {
            if binding.action.eq(action)
                && binding
                    .context_predicate
                    .as_ref()
                    .map_or(true, |predicate| predicate.eval(cx))
            {
                return Some(binding.keystrokes.clone());
            }
        }
        None
    }

    pub fn available_bindings(
        &self,
        view_id: usize,
        context: &Context,
    ) -> BTreeMap<SmallVec<[Keystroke; 2]>, &Binding> {
        let mut result: BTreeMap<SmallVec<[Keystroke; 2]>, &Binding> = Default::default();

        let pending_keystrokes = self
            .pending
            .get(&view_id)
            .map(|p| p.keystrokes.clone())
            .unwrap_or_default();

        for binding in self.keymap.bindings.iter().rev() {
            if binding.keystrokes.starts_with(&pending_keystrokes)
                && binding
                    .context_predicate
                    .as_ref()
                    .map(|c| c.eval(context))
                    .unwrap_or(true)
            {
                let next_keystrokes = binding
                    .keystrokes
                    .iter()
                    .skip(pending_keystrokes.len())
                    .cloned()
                    .collect();
                if !result.contains_key(&next_keystrokes) {
                    result.insert(next_keystrokes, binding);
                }
            }
        }

        result
    }
}

impl Default for Matcher {
    fn default() -> Self {
        Self::new(Keymap::default())
    }
}

impl Keymap {
    pub fn new(bindings: Vec<Binding>) -> Self {
        let mut binding_indices_by_action_type = HashMap::new();
        for (ix, binding) in bindings.iter().enumerate() {
            binding_indices_by_action_type
                .entry(binding.action.as_any().type_id())
                .or_insert_with(SmallVec::new)
                .push(ix);
        }
        Self {
            binding_indices_by_action_type,
            bindings,
        }
    }

    fn bindings_for_action_type(&self, action_type: TypeId) -> impl Iterator<Item = &'_ Binding> {
        self.binding_indices_by_action_type
            .get(&action_type)
            .map(SmallVec::as_slice)
            .unwrap_or(&[])
            .iter()
            .map(|ix| &self.bindings[*ix])
    }

    fn add_bindings<T: IntoIterator<Item = Binding>>(&mut self, bindings: T) {
        for binding in bindings {
            self.binding_indices_by_action_type
                .entry(binding.action.as_any().type_id())
                .or_default()
                .push(self.bindings.len());
            self.bindings.push(binding);
        }
    }

    fn clear(&mut self) {
        self.bindings.clear();
        self.binding_indices_by_action_type.clear();
    }
}

impl Binding {
    pub fn new<A: Action>(keystrokes: &str, action: A, context: Option<&str>) -> Self {
        Self::load(keystrokes, Box::new(action), context).unwrap()
    }

    pub fn load(keystrokes: &str, action: Box<dyn Action>, context: Option<&str>) -> Result<Self> {
        let context = if let Some(context) = context {
            Some(ContextPredicate::parse(context)?)
        } else {
            None
        };

        let keystrokes = keystrokes
            .split_whitespace()
            .map(Keystroke::parse)
            .collect::<Result<_>>()?;

        Ok(Self {
            keystrokes,
            action,
            context_predicate: context,
        })
    }

    pub fn keystrokes(&self) -> &[Keystroke] {
        &self.keystrokes
    }

    pub fn action(&self) -> &dyn Action {
        self.action.as_ref()
    }

    pub fn context_contains_identifier(&self, id: &str) -> bool {
        self.context_predicate
            .as_ref()
            .map(|pred| pred.contains_identifier(id))
            .unwrap_or(false)
    }
}

impl Keystroke {
    pub fn parse(source: &str) -> anyhow::Result<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut cmd = false;
        let mut function = false;
        let mut key = None;

        let mut components = source.split('-').peekable();
        while let Some(component) = components.next() {
            match component {
                "ctrl" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                "cmd" => cmd = true,
                "fn" => function = true,
                _ => {
                    if let Some(component) = components.peek() {
                        if component.is_empty() && source.ends_with('-') {
                            key = Some(String::from("-"));
                            break;
                        } else {
                            return Err(anyhow!("Invalid keystroke `{}`", source));
                        }
                    } else {
                        key = Some(String::from(component));
                    }
                }
            }
        }

        let key = key.ok_or_else(|| anyhow!("Invalid keystroke `{}`", source))?;

        Ok(Keystroke {
            ctrl,
            alt,
            shift,
            cmd,
            function,
            key,
        })
    }

    pub fn modified(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.cmd
    }
}

impl std::fmt::Display for Keystroke {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.ctrl {
            f.write_char('^')?;
        }
        if self.alt {
            f.write_char('⎇')?;
        }
        if self.cmd {
            f.write_char('⌘')?;
        }
        if self.shift {
            f.write_char('⇧')?;
        }
        let key = match self.key.as_str() {
            "backspace" => '⌫',
            "up" => '↑',
            "down" => '↓',
            "left" => '←',
            "right" => '→',
            "tab" => '⇥',
            "escape" => '⎋',
            key => {
                if key.len() == 1 {
                    key.chars().next().unwrap().to_ascii_uppercase()
                } else {
                    return f.write_str(key);
                }
            }
        };
        f.write_char(key)
    }
}

impl Context {
    pub fn extend(&mut self, other: &Context) {
        for v in &other.set {
            self.set.insert(v.clone());
        }
        for (k, v) in &other.map {
            self.map.insert(k.clone(), v.clone());
        }
    }
}

impl ContextPredicate {
    fn parse(source: &str) -> anyhow::Result<Self> {
        let mut parser = Parser::new();
        let language = unsafe { tree_sitter_context_predicate() };
        parser.set_language(language).unwrap();
        let source = source.as_bytes();
        let tree = parser.parse(source, None).unwrap();
        Self::from_node(tree.root_node(), source)
    }

    fn from_node(node: Node, source: &[u8]) -> anyhow::Result<Self> {
        let parse_error = "error parsing context predicate";
        let kind = node.kind();

        match kind {
            "source" => Self::from_node(node.child(0).ok_or_else(|| anyhow!(parse_error))?, source),
            "identifier" => Ok(Self::Identifier(node.utf8_text(source)?.into())),
            "not" => {
                let child = Self::from_node(
                    node.child_by_field_name("expression")
                        .ok_or_else(|| anyhow!(parse_error))?,
                    source,
                )?;
                Ok(Self::Not(Box::new(child)))
            }
            "and" | "or" => {
                let left = Box::new(Self::from_node(
                    node.child_by_field_name("left")
                        .ok_or_else(|| anyhow!(parse_error))?,
                    source,
                )?);
                let right = Box::new(Self::from_node(
                    node.child_by_field_name("right")
                        .ok_or_else(|| anyhow!(parse_error))?,
                    source,
                )?);
                if kind == "and" {
                    Ok(Self::And(left, right))
                } else {
                    Ok(Self::Or(left, right))
                }
            }
            "equal" | "not_equal" => {
                let left = node
                    .child_by_field_name("left")
                    .ok_or_else(|| anyhow!(parse_error))?
                    .utf8_text(source)?
                    .into();
                let right = node
                    .child_by_field_name("right")
                    .ok_or_else(|| anyhow!(parse_error))?
                    .utf8_text(source)?
                    .into();
                if kind == "equal" {
                    Ok(Self::Equal(left, right))
                } else {
                    Ok(Self::NotEqual(left, right))
                }
            }
            "parenthesized" => Self::from_node(
                node.child_by_field_name("expression")
                    .ok_or_else(|| anyhow!(parse_error))?,
                source,
            ),
            _ => Err(anyhow!(parse_error)),
        }
    }

    fn eval(&self, cx: &Context) -> bool {
        match self {
            Self::Identifier(name) => cx.set.contains(name.as_str()),
            Self::Equal(left, right) => cx
                .map
                .get(left)
                .map(|value| value == right)
                .unwrap_or(false),
            Self::NotEqual(left, right) => {
                cx.map.get(left).map(|value| value != right).unwrap_or(true)
            }
            Self::Not(pred) => !pred.eval(cx),
            Self::And(left, right) => left.eval(cx) && right.eval(cx),
            Self::Or(left, right) => left.eval(cx) || right.eval(cx),
        }
    }

    fn contains_identifier(&self, id: &str) -> bool {
        match self {
            Self::Identifier(name) => name == id,
            Self::Not(pred) => pred.contains_identifier(id),
            Self::And(left, right) => left.contains_identifier(id) || right.contains_identifier(id),
            Self::Or(left, right) => left.contains_identifier(id) || right.contains_identifier(id),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use serde::Deserialize;

    use crate::{actions, impl_actions};

    use super::*;

    #[test]
    fn test_push_keystroke() -> Result<()> {
        actions!(test, [B, AB, C]);

        let mut ctx1 = Context::default();
        ctx1.set.insert("1".into());

        let mut ctx2 = Context::default();
        ctx2.set.insert("2".into());

        let dispatch_path = vec![(2, ctx2), (1, ctx1)];

        let keymap = Keymap::new(vec![
            Binding::new("a b", AB, Some("1")),
            Binding::new("b", B, Some("2")),
            Binding::new("c", C, Some("2")),
        ]);

        let mut matcher = Matcher::new(keymap);

        assert_eq!(
            MatchResult::Pending,
            matcher.push_keystroke(Keystroke::parse("a")?, dispatch_path.clone())
        );
        assert_eq!(
            MatchResult::Match {
                view_id: 1,
                action: Box::new(AB)
            },
            matcher.push_keystroke(Keystroke::parse("b")?, dispatch_path.clone())
        );
        assert!(matcher.pending.is_empty());
        assert_eq!(
            MatchResult::Match {
                view_id: 2,
                action: Box::new(B)
            },
            matcher.push_keystroke(Keystroke::parse("b")?, dispatch_path.clone())
        );
        assert!(matcher.pending.is_empty());
        assert_eq!(
            MatchResult::Pending,
            matcher.push_keystroke(Keystroke::parse("a")?, dispatch_path.clone())
        );
        assert_eq!(
            MatchResult::None,
            matcher.push_keystroke(Keystroke::parse("c")?, dispatch_path.clone())
        );
        assert!(matcher.pending.is_empty());

        Ok(())
    }

    #[test]
    fn test_keystroke_parsing() -> Result<()> {
        assert_eq!(
            Keystroke::parse("ctrl-p")?,
            Keystroke {
                key: "p".into(),
                ctrl: true,
                alt: false,
                shift: false,
                cmd: false,
                function: false,
            }
        );

        assert_eq!(
            Keystroke::parse("alt-shift-down")?,
            Keystroke {
                key: "down".into(),
                ctrl: false,
                alt: true,
                shift: true,
                cmd: false,
                function: false,
            }
        );

        assert_eq!(
            Keystroke::parse("shift-cmd--")?,
            Keystroke {
                key: "-".into(),
                ctrl: false,
                alt: false,
                shift: true,
                cmd: true,
                function: false,
            }
        );

        Ok(())
    }

    #[test]
    fn test_context_predicate_parsing() -> Result<()> {
        use ContextPredicate::*;

        assert_eq!(
            ContextPredicate::parse("a && (b == c || d != e)")?,
            And(
                Box::new(Identifier("a".into())),
                Box::new(Or(
                    Box::new(Equal("b".into(), "c".into())),
                    Box::new(NotEqual("d".into(), "e".into())),
                ))
            )
        );

        assert_eq!(
            ContextPredicate::parse("!a")?,
            Not(Box::new(Identifier("a".into())),)
        );

        Ok(())
    }

    #[test]
    fn test_context_predicate_eval() -> Result<()> {
        let predicate = ContextPredicate::parse("a && b || c == d")?;

        let mut context = Context::default();
        context.set.insert("a".into());
        assert!(!predicate.eval(&context));

        context.set.insert("b".into());
        assert!(predicate.eval(&context));

        context.set.remove("b");
        context.map.insert("c".into(), "x".into());
        assert!(!predicate.eval(&context));

        context.map.insert("c".into(), "d".into());
        assert!(predicate.eval(&context));

        let predicate = ContextPredicate::parse("!a")?;
        assert!(predicate.eval(&Context::default()));

        Ok(())
    }

    #[test]
    fn test_matcher() -> Result<()> {
        #[derive(Clone, Deserialize, PartialEq, Eq, Debug)]
        pub struct A(pub String);
        impl_actions!(test, [A]);
        actions!(test, [B, Ab]);

        #[derive(Clone, Debug, Eq, PartialEq)]
        struct ActionArg {
            a: &'static str,
        }

        let keymap = Keymap::new(vec![
            Binding::new("a", A("x".to_string()), Some("a")),
            Binding::new("b", B, Some("a")),
            Binding::new("a b", Ab, Some("a || b")),
        ]);

        let mut ctx_a = Context::default();
        ctx_a.set.insert("a".into());

        let mut ctx_b = Context::default();
        ctx_b.set.insert("b".into());

        let mut matcher = Matcher::new(keymap);

        // Basic match
        assert_eq!(
            downcast(&matcher.test_keystroke("a", vec![(1, ctx_a.clone())])),
            Some(&A("x".to_string()))
        );

        // Multi-keystroke match
        assert!(matcher
            .test_keystroke("a", vec![(1, ctx_b.clone())])
            .is_none());
        assert_eq!(
            downcast(&matcher.test_keystroke("b", vec![(1, ctx_b.clone())])),
            Some(&Ab)
        );

        // Failed matches don't interfere with matching subsequent keys
        assert!(matcher
            .test_keystroke("x", vec![(1, ctx_a.clone())])
            .is_none());
        assert_eq!(
            downcast(&matcher.test_keystroke("a", vec![(1, ctx_a.clone())])),
            Some(&A("x".to_string()))
        );

        // Pending keystrokes are cleared when the context changes
        assert!(&matcher
            .test_keystroke("a", vec![(1, ctx_b.clone())])
            .is_none());
        assert_eq!(
            downcast(&matcher.test_keystroke("b", vec![(1, ctx_a.clone())])),
            Some(&B)
        );

        let mut ctx_c = Context::default();
        ctx_c.set.insert("c".into());

        // Pending keystrokes are maintained per-view
        assert!(matcher
            .test_keystroke("a", vec![(1, ctx_b.clone()), (2, ctx_c.clone())])
            .is_none());
        assert_eq!(
            downcast(&matcher.test_keystroke("b", vec![(1, ctx_b.clone())])),
            Some(&Ab)
        );

        Ok(())
    }

    fn downcast<A: Action>(action: &Option<Box<dyn Action>>) -> Option<&A> {
        action
            .as_ref()
            .and_then(|action| action.as_any().downcast_ref())
    }

    impl Matcher {
        fn test_keystroke(
            &mut self,
            keystroke: &str,
            dispatch_path: Vec<(usize, Context)>,
        ) -> Option<Box<dyn Action>> {
            if let MatchResult::Match { action, .. } =
                self.push_keystroke(Keystroke::parse(keystroke).unwrap(), dispatch_path)
            {
                Some(action.boxed_clone())
            } else {
                None
            }
        }
    }
}
