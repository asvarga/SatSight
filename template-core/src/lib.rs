//! Core library for the template workspace.
//!
//! This is the reusable, frontend-free layer. The GUI crate (`template-gui`)
//! depends on it and renders whatever it exposes. Replace the placeholder below
//! with your own logic.

/// A friendly greeting, used by the GUI as a placeholder.
#[must_use]
pub fn greeting(name: &str) -> String {
    format!("Hello, {name}!")
}

#[cfg(test)]
mod tests {
    use super::greeting;

    #[test]
    fn greeting_includes_the_name() {
        assert_eq!(greeting("world"), "Hello, world!");
    }
}
