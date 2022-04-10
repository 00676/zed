use anyhow::Result;
use collections::BTreeMap;
use gpui::{keymap::Binding, MutableAppContext};
use serde::Deserialize;
use serde_json::value::RawValue;

#[derive(Deserialize)]
struct ActionWithData<'a>(#[serde(borrow)] &'a str, #[serde(borrow)] &'a RawValue);
type ActionSetsByContext<'a> = BTreeMap<&'a str, ActionsByKeystroke<'a>>;
type ActionsByKeystroke<'a> = BTreeMap<&'a str, &'a RawValue>;

pub fn load_keymap(cx: &mut MutableAppContext, content: &str) -> Result<()> {
    let actions: ActionSetsByContext = serde_json::from_str(content)?;
    for (context, actions) in actions {
        let context = if context.is_empty() {
            None
        } else {
            Some(context)
        };
        cx.add_bindings(
            actions
                .into_iter()
                .map(|(keystroke, action)| {
                    let action = action.get();
                    let action = if action.starts_with('[') {
                        let ActionWithData(name, data) = serde_json::from_str(action)?;
                        cx.deserialize_action(name, Some(data.get()))
                    } else {
                        let name = serde_json::from_str(action)?;
                        cx.deserialize_action(name, None)
                    }?;
                    Binding::load(keystroke, action, context)
                })
                .collect::<Result<Vec<_>>>()?,
        )
    }
    Ok(())
}
