//! Moteur de style XAML, générique sur l'arbre visuel.
//!
//! Toute la logique vit ici : correspondance des sélecteurs, états visuels,
//! variables, journal des valeurs d'origine et disjoncteur. Le trait
//! [`XamlBackend`] isole l'accès à Windows, ce qui permet de tester le moteur
//! avec un arbre simulé.

mod engine;

pub use engine::{Engine, Mode, RuleStat, XamlBackend, MAX_WRITE_FAILURES};

#[cfg(test)]
mod tests;
