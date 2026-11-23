//! Calcul de la taille des miniatures, repris de Taskbar Thumbnail Size.

use serde::{Deserialize, Serialize};

/// Compatible ABI avec `Windows.Foundation.Size`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Percent,
    Absolute,
}

/// Réglages des miniatures. Un minimum ou maximum à 0 est désactivé.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThumbnailConfig {
    pub enabled: bool,
    pub mode: Mode,
    pub percent: u32,
    pub min_width: u32,
    pub min_height: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub preserve_aspect_ratio: bool,
}

impl Default for ThumbnailConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: Mode::Percent,
            percent: 150,
            min_width: 180,
            min_height: 120,
            max_width: 180,
            max_height: 120,
            preserve_aspect_ratio: true,
        }
    }
}

impl ThumbnailConfig {
    /// Refuse les valeurs qui donneraient une miniature inutilisable.
    pub fn validate(&self) -> Result<(), String> {
        if !(10..=1000).contains(&self.percent) {
            return Err(format!(
                "percent doit être entre 10 et 1000 (reçu : {})",
                self.percent
            ));
        }
        for (name, value) in [
            ("min_width", self.min_width),
            ("min_height", self.min_height),
            ("max_width", self.max_width),
            ("max_height", self.max_height),
        ] {
            if value > 8192 {
                return Err(format!("{name} doit être au plus 8192 (reçu : {value})"));
            }
        }
        Ok(())
    }

    /// Calcule la taille finale.
    ///
    /// `original` appelle la fonction de Windows avec l'échelle donnée.
    pub fn compute(&self, scale: f32, mut original: impl FnMut(f32) -> Size) -> Size {
        if self.mode == Mode::Percent {
            return original(scale * self.percent as f32 / 100.0);
        }

        let current = original(scale);
        if current.width <= 0.0 || current.height <= 0.0 {
            return current;
        }

        if self.preserve_aspect_ratio {
            let factor = self.fit_factor(current);
            return if factor == 1.0 {
                current
            } else {
                original(scale * factor)
            };
        }

        Size {
            width: clamp_dimension(current.width, self.min_width, self.max_width),
            height: clamp_dimension(current.height, self.min_height, self.max_height),
        }
    }

    /// Facteur le plus proche de 1 respectant les contraintes. Le maximum prime.
    fn fit_factor(&self, current: Size) -> f32 {
        let lower = |limit: u32, value: f32| if limit > 0 { limit as f32 / value } else { 1.0 };
        let upper = |limit: u32, value: f32| {
            if limit > 0 {
                limit as f32 / value
            } else {
                f32::INFINITY
            }
        };
        let required =
            lower(self.min_width, current.width).max(lower(self.min_height, current.height));
        let allowed =
            upper(self.max_width, current.width).min(upper(self.max_height, current.height));

        if allowed < required {
            allowed
        } else if required > 1.0 {
            required
        } else if allowed < 1.0 {
            allowed
        } else {
            1.0
        }
    }
}

fn clamp_dimension(mut value: f32, minimum: u32, maximum: u32) -> f32 {
    if minimum > 0 && value < minimum as f32 {
        value = minimum as f32;
    }
    if maximum > 0 && value > maximum as f32 {
        value = maximum as f32;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simule Windows : taille de base 200x100, proportionnelle à l'échelle.
    fn windows(scale: f32) -> Size {
        Size {
            width: 200.0 * scale,
            height: 100.0 * scale,
        }
    }

    fn absolute(min: (u32, u32), max: (u32, u32), keep: bool) -> ThumbnailConfig {
        ThumbnailConfig {
            enabled: true,
            mode: Mode::Absolute,
            min_width: min.0,
            min_height: min.1,
            max_width: max.0,
            max_height: max.1,
            preserve_aspect_ratio: keep,
            ..ThumbnailConfig::default()
        }
    }

    #[test]
    fn percent_scales_the_input() {
        let config = ThumbnailConfig::default();
        assert_eq!(
            config.compute(1.0, windows),
            Size {
                width: 300.0,
                height: 150.0
            }
        );
    }

    #[test]
    fn absolute_keeps_ratio_when_shrinking() {
        let result = absolute((0, 0), (100, 0), true).compute(1.0, windows);
        assert_eq!(
            result,
            Size {
                width: 100.0,
                height: 50.0
            }
        );
    }

    #[test]
    fn absolute_keeps_ratio_when_growing() {
        let result = absolute((400, 0), (0, 0), true).compute(1.0, windows);
        assert_eq!(
            result,
            Size {
                width: 400.0,
                height: 200.0
            }
        );
    }

    #[test]
    fn satisfied_constraints_avoid_a_second_call() {
        let mut calls = 0;
        let result = absolute((10, 10), (1000, 1000), true).compute(1.0, |s| {
            calls += 1;
            windows(s)
        });
        assert_eq!(result, windows(1.0));
        assert_eq!(calls, 1);
    }

    #[test]
    fn conflicting_constraints_favor_the_maximum() {
        let result = absolute((400, 0), (0, 50), true).compute(1.0, windows);
        assert_eq!(
            result,
            Size {
                width: 100.0,
                height: 50.0
            }
        );
    }

    #[test]
    fn stretching_without_ratio() {
        let result = absolute((180, 120), (180, 120), false).compute(1.0, windows);
        assert_eq!(
            result,
            Size {
                width: 180.0,
                height: 120.0
            }
        );
    }

    #[test]
    fn invalid_original_size_is_returned_as_is() {
        let zero = Size {
            width: 0.0,
            height: 0.0,
        };
        assert_eq!(
            absolute((10, 10), (20, 20), true).compute(1.0, |_| zero),
            zero
        );
    }

    #[test]
    fn validation_bounds() {
        let mut config = ThumbnailConfig::default();
        assert!(config.validate().is_ok());
        config.percent = 0;
        assert!(config.validate().is_err());
        config.percent = 100;
        config.max_width = 9000;
        assert!(config.validate().is_err());
    }
}
