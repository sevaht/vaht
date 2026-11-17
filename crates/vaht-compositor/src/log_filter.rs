//! Which events vaht records, changed while it runs (`vaht msg
//! log-filter`): the filter lives with the logging, which the compositor
//! reaches through [`LogFilterControl`], since logging is set up first.

use std::fmt;
use std::sync::Arc;

/// What can be asked of the log filter.
pub trait LogFilterControl: fmt::Debug + Send + Sync {
    /// Returns the directives in effect.
    fn directives(&self) -> String;

    /// Replaces the filter with `directives`.
    ///
    /// # Errors
    ///
    /// Returns why not, the filter unchanged: the directives are invalid.
    fn set(&self, directives: &str) -> Result<(), String>;

    /// Goes back to the filter vaht started with.
    ///
    /// # Errors
    ///
    /// Returns why not: the logging is gone, as vaht exits.
    fn reset(&self) -> Result<(), String>;
}

/// The log filter, shared.
#[derive(Debug, Clone)]
pub struct LogFilterHandle(Arc<dyn LogFilterControl>);

impl LogFilterHandle {
    /// Returns a handle on `control`.
    pub fn new(control: impl LogFilterControl + 'static) -> Self {
        Self(Arc::new(control))
    }

    /// Changes the filter to `directives`, or back to the one vaht started
    /// with if `reset`, or not at all with neither; returns the filter in
    /// effect.
    ///
    /// # Errors
    ///
    /// Returns why not: both were asked, or the change failed.
    pub(crate) fn change(&self, directives: Option<&str>, reset: bool) -> Result<String, String> {
        match (directives, reset) {
            (Some(_), true) => {
                return Err("give a filter or reset, not both".to_owned());
            }
            (Some(directives), false) => self.0.set(directives)?,
            (None, true) => self.0.reset()?,
            (None, false) => {}
        }
        Ok(self.0.directives())
    }
}

impl PartialEq for LogFilterHandle {
    /// Handles are the same when they reach the same filter.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for LogFilterHandle {}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A filter that remembers what it was set to, refusing "bad".
    #[derive(Debug)]
    struct Remembering(Mutex<String>);

    impl LogFilterControl for Remembering {
        fn directives(&self) -> String {
            self.0.lock().expect("not poisoned").clone()
        }

        fn set(&self, directives: &str) -> Result<(), String> {
            if directives == "bad" {
                return Err("not a filter".to_owned());
            }
            directives.clone_into(&mut self.0.lock().expect("not poisoned"));
            Ok(())
        }

        fn reset(&self) -> Result<(), String> {
            "info".clone_into(&mut self.0.lock().expect("not poisoned"));
            Ok(())
        }
    }

    #[test]
    fn a_filter_is_told_set_and_reset() {
        let handle = LogFilterHandle::new(Remembering(Mutex::new("info".to_owned())));
        assert_eq!(handle.change(None, false), Ok("info".to_owned()));
        assert_eq!(handle.change(Some("debug"), false), Ok("debug".to_owned()));
        assert_eq!(handle.change(None, true), Ok("info".to_owned()));
    }

    #[test]
    fn a_bad_filter_or_both_at_once_is_refused() {
        let handle = LogFilterHandle::new(Remembering(Mutex::new("info".to_owned())));
        assert_eq!(
            handle.change(Some("bad"), false),
            Err("not a filter".to_owned())
        );
        assert!(handle.change(Some("debug"), true).is_err());
        assert_eq!(
            handle.change(None, false),
            Ok("info".to_owned()),
            "unchanged"
        );
    }
}
