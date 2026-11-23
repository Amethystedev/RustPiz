//! Moteur de thèmes de RustPiz : indépendant de Windows et testable partout.
//!
//! Les chaînes `target` et `styles` gardent exactement la syntaxe du
//! Windows 11 Taskbar Styler.

pub mod compile;
pub mod expr;
pub mod model;
pub mod resource;
pub mod rule;
pub mod selector;
pub mod validate;

pub use compile::{compile, CompiledRule, CompiledTheme};
pub use model::{ControlStyle, Theme, ThemeError};
pub use validate::{check_build, validate, Diagnostic, Severity};
