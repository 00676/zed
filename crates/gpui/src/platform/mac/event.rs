use crate::{
    geometry::vector::vec2f,
    keymap::Keystroke,
    platform::{Event, NavigationDirection},
    KeyDownEvent, KeyUpEvent, ModifiersChangedEvent, MouseButton, MouseEvent, MouseMovedEvent,
    ScrollWheelEvent,
};
use cocoa::{
    appkit::{NSEvent, NSEventModifierFlags, NSEventType},
    base::{id, YES},
    foundation::NSString as _,
};
use std::{borrow::Cow, ffi::CStr, os::raw::c_char};

const BACKSPACE_KEY: u16 = 0x7f;
const SPACE_KEY: u16 = b' ' as u16;
const ENTER_KEY: u16 = 0x0d;
const NUMPAD_ENTER_KEY: u16 = 0x03;
const ESCAPE_KEY: u16 = 0x1b;
const TAB_KEY: u16 = 0x09;
const SHIFT_TAB_KEY: u16 = 0x19;

pub fn key_to_native(key: &str) -> Cow<str> {
    use cocoa::appkit::*;
    let code = match key {
        "space" => SPACE_KEY,
        "backspace" => BACKSPACE_KEY,
        "up" => NSUpArrowFunctionKey,
        "down" => NSDownArrowFunctionKey,
        "left" => NSLeftArrowFunctionKey,
        "right" => NSRightArrowFunctionKey,
        "pageup" => NSPageUpFunctionKey,
        "pagedown" => NSPageDownFunctionKey,
        "delete" => NSDeleteFunctionKey,
        "f1" => NSF1FunctionKey,
        "f2" => NSF2FunctionKey,
        "f3" => NSF3FunctionKey,
        "f4" => NSF4FunctionKey,
        "f5" => NSF5FunctionKey,
        "f6" => NSF6FunctionKey,
        "f7" => NSF7FunctionKey,
        "f8" => NSF8FunctionKey,
        "f9" => NSF9FunctionKey,
        "f10" => NSF10FunctionKey,
        "f11" => NSF11FunctionKey,
        "f12" => NSF12FunctionKey,
        _ => return Cow::Borrowed(key),
    };
    Cow::Owned(String::from_utf16(&[code]).unwrap())
}

impl Event {
    pub unsafe fn from_native(native_event: id, window_height: Option<f32>) -> Option<Self> {
        let event_type = native_event.eventType();

        // Filter out event types that aren't in the NSEventType enum.
        // See https://github.com/servo/cocoa-rs/issues/155#issuecomment-323482792 for details.
        match event_type as u64 {
            0 | 21 | 32 | 33 | 35 | 36 | 37 => {
                return None;
            }
            _ => {}
        }

        match event_type {
            NSEventType::NSFlagsChanged => {
                let modifiers = native_event.modifierFlags();
                let ctrl = modifiers.contains(NSEventModifierFlags::NSControlKeyMask);
                let alt = modifiers.contains(NSEventModifierFlags::NSAlternateKeyMask);
                let shift = modifiers.contains(NSEventModifierFlags::NSShiftKeyMask);
                let cmd = modifiers.contains(NSEventModifierFlags::NSCommandKeyMask);

                Some(Self::ModifiersChanged(ModifiersChangedEvent {
                    ctrl,
                    alt,
                    shift,
                    cmd,
                }))
            }
            NSEventType::NSKeyDown => {
                let modifiers = native_event.modifierFlags();
                let ctrl = modifiers.contains(NSEventModifierFlags::NSControlKeyMask);
                let alt = modifiers.contains(NSEventModifierFlags::NSAlternateKeyMask);
                let shift = modifiers.contains(NSEventModifierFlags::NSShiftKeyMask);
                let cmd = modifiers.contains(NSEventModifierFlags::NSCommandKeyMask);

                let unmodified_chars = get_key_text(native_event)?;
                Some(Self::KeyDown(KeyDownEvent {
                    keystroke: Keystroke {
                        ctrl,
                        alt,
                        shift,
                        cmd,
                        key: unmodified_chars.into(),
                    },
                    is_held: native_event.isARepeat() == YES,
                }))
            }
            NSEventType::NSKeyUp => {
                let modifiers = native_event.modifierFlags();
                let ctrl = modifiers.contains(NSEventModifierFlags::NSControlKeyMask);
                let alt = modifiers.contains(NSEventModifierFlags::NSAlternateKeyMask);
                let shift = modifiers.contains(NSEventModifierFlags::NSShiftKeyMask);
                let cmd = modifiers.contains(NSEventModifierFlags::NSCommandKeyMask);
                let unmodified_chars = get_key_text(native_event)?;
                Some(Self::KeyUp(KeyUpEvent {
                    keystroke: Keystroke {
                        ctrl,
                        alt,
                        shift,
                        cmd,
                        key: unmodified_chars.into(),
                    },
                }))
            }
            NSEventType::NSLeftMouseDown
            | NSEventType::NSRightMouseDown
            | NSEventType::NSOtherMouseDown => {
                let button = match native_event.buttonNumber() {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Navigate(NavigationDirection::Back),
                    4 => MouseButton::Navigate(NavigationDirection::Forward),
                    // Other mouse buttons aren't tracked currently
                    _ => return None,
                };
                let modifiers = native_event.modifierFlags();

                window_height.map(|window_height| {
                    Self::MouseDown(MouseEvent {
                        button,
                        position: vec2f(
                            native_event.locationInWindow().x as f32,
                            window_height - native_event.locationInWindow().y as f32,
                        ),
                        ctrl: modifiers.contains(NSEventModifierFlags::NSControlKeyMask),
                        alt: modifiers.contains(NSEventModifierFlags::NSAlternateKeyMask),
                        shift: modifiers.contains(NSEventModifierFlags::NSShiftKeyMask),
                        cmd: modifiers.contains(NSEventModifierFlags::NSCommandKeyMask),
                        click_count: native_event.clickCount() as usize,
                    })
                })
            }
            NSEventType::NSLeftMouseUp
            | NSEventType::NSRightMouseUp
            | NSEventType::NSOtherMouseUp => {
                let button = match native_event.buttonNumber() {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Navigate(NavigationDirection::Back),
                    4 => MouseButton::Navigate(NavigationDirection::Forward),
                    // Other mouse buttons aren't tracked currently
                    _ => return None,
                };

                window_height.map(|window_height| {
                    let modifiers = native_event.modifierFlags();
                    Self::MouseUp(MouseEvent {
                        button,
                        position: vec2f(
                            native_event.locationInWindow().x as f32,
                            window_height - native_event.locationInWindow().y as f32,
                        ),
                        ctrl: modifiers.contains(NSEventModifierFlags::NSControlKeyMask),
                        alt: modifiers.contains(NSEventModifierFlags::NSAlternateKeyMask),
                        shift: modifiers.contains(NSEventModifierFlags::NSShiftKeyMask),
                        cmd: modifiers.contains(NSEventModifierFlags::NSCommandKeyMask),
                        click_count: native_event.clickCount() as usize,
                    })
                })
            }
            NSEventType::NSScrollWheel => window_height.map(|window_height| {
                Self::ScrollWheel(ScrollWheelEvent {
                    position: vec2f(
                        native_event.locationInWindow().x as f32,
                        window_height - native_event.locationInWindow().y as f32,
                    ),
                    delta: vec2f(
                        native_event.scrollingDeltaX() as f32,
                        native_event.scrollingDeltaY() as f32,
                    ),
                    precise: native_event.hasPreciseScrollingDeltas() == YES,
                })
            }),
            NSEventType::NSLeftMouseDragged
            | NSEventType::NSRightMouseDragged
            | NSEventType::NSOtherMouseDragged => {
                let pressed_button = match native_event.buttonNumber() {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Navigate(NavigationDirection::Back),
                    4 => MouseButton::Navigate(NavigationDirection::Forward),
                    // Other mouse buttons aren't tracked currently
                    _ => return None,
                };

                window_height.map(|window_height| {
                    let modifiers = native_event.modifierFlags();
                    Self::MouseMoved(MouseMovedEvent {
                        pressed_button: Some(pressed_button),
                        position: vec2f(
                            native_event.locationInWindow().x as f32,
                            window_height - native_event.locationInWindow().y as f32,
                        ),
                        ctrl: modifiers.contains(NSEventModifierFlags::NSControlKeyMask),
                        alt: modifiers.contains(NSEventModifierFlags::NSAlternateKeyMask),
                        shift: modifiers.contains(NSEventModifierFlags::NSShiftKeyMask),
                        cmd: modifiers.contains(NSEventModifierFlags::NSCommandKeyMask),
                    })
                })
            }
            NSEventType::NSMouseMoved => window_height.map(|window_height| {
                let modifiers = native_event.modifierFlags();
                Self::MouseMoved(MouseMovedEvent {
                    position: vec2f(
                        native_event.locationInWindow().x as f32,
                        window_height - native_event.locationInWindow().y as f32,
                    ),
                    pressed_button: None,
                    ctrl: modifiers.contains(NSEventModifierFlags::NSControlKeyMask),
                    alt: modifiers.contains(NSEventModifierFlags::NSAlternateKeyMask),
                    shift: modifiers.contains(NSEventModifierFlags::NSShiftKeyMask),
                    cmd: modifiers.contains(NSEventModifierFlags::NSCommandKeyMask),
                })
            }),
            _ => None,
        }
    }
}

unsafe fn get_key_text(native_event: id) -> Option<&'static str> {
    let unmodified_chars =
        CStr::from_ptr(native_event.charactersIgnoringModifiers().UTF8String() as *mut c_char)
            .to_str()
            .unwrap();

    let first_char = unmodified_chars.chars().next()?;
    use cocoa::appkit::*;

    #[allow(non_upper_case_globals)]
    let unmodified_chars = match first_char as u16 {
        SPACE_KEY => "space",
        BACKSPACE_KEY => "backspace",
        ENTER_KEY | NUMPAD_ENTER_KEY => "enter",
        ESCAPE_KEY => "escape",
        TAB_KEY => "tab",
        SHIFT_TAB_KEY => "tab",

        NSUpArrowFunctionKey => "up",
        NSDownArrowFunctionKey => "down",
        NSLeftArrowFunctionKey => "left",
        NSRightArrowFunctionKey => "right",
        NSPageUpFunctionKey => "pageup",
        NSPageDownFunctionKey => "pagedown",
        NSDeleteFunctionKey => "delete",
        NSF1FunctionKey => "f1",
        NSF2FunctionKey => "f2",
        NSF3FunctionKey => "f3",
        NSF4FunctionKey => "f4",
        NSF5FunctionKey => "f5",
        NSF6FunctionKey => "f6",
        NSF7FunctionKey => "f7",
        NSF8FunctionKey => "f8",
        NSF9FunctionKey => "f9",
        NSF10FunctionKey => "f10",
        NSF11FunctionKey => "f11",
        NSF12FunctionKey => "f12",
        _ => unmodified_chars,
    };

    Some(unmodified_chars)
}
