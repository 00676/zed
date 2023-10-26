use gpui2::Element;

pub trait ElementExt<S: 'static + Send + Sync>: Element<S> {
    /// Applies a given function `then` to the current element if `condition` is true.
    /// This function is used to conditionally modify the element based on a given condition.
    /// If `condition` is false, it just returns the current element as it is.
    fn when(mut self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self
    where
        Self: Sized,
    {
        if condition {
            self = then(self);
        }
        self
    }

    // fn when_some<T, U>(mut self, option: Option<T>, then: impl FnOnce(Self, T) -> U) -> U
    // where
    //     Self: Sized,
    // {
    //     if let Some(value) = option {
    //         self = then(self, value);
    //     }
    //     self
    // }
}

impl<S: 'static + Send + Sync, E: Element<S>> ElementExt<S> for E {}
